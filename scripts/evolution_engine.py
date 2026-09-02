#!/usr/bin/env python3
"""Evidence-driven, reversible routing evolution for Super-Instruct.

The controller learns only from completed interaction records. It proposes
skills whose observed score exceeds the category baseline, validates the
candidate against schema and regression invariants, then atomically promotes
the policy. Every generation remains available for audit and rollback.
"""

from __future__ import annotations

import argparse
import contextlib
import fcntl
import hashlib
import json
import os
import pathlib
import shutil
import tempfile
from collections import defaultdict
from datetime import datetime, timezone


ROOT = pathlib.Path(__file__).resolve().parents[1]
DEFAULT_CONFIG = ROOT / "evolution" / "config.json"
DEFAULT_POLICY = ROOT / "evolution" / "policy.json"
DEFAULT_HISTORY = ROOT / "evolution" / "generations"
ALIGNMENT_FAILURES = {"model_refusal", "task_divergence", "provider_policy_block"}


def read_json(path: pathlib.Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def file_sha256(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


@contextlib.contextmanager
def exclusive_lock(policy_path: pathlib.Path):
    lock_path = policy_path.with_suffix(".lock")
    lock_path.parent.mkdir(parents=True, exist_ok=True)
    with lock_path.open("a+", encoding="utf-8") as stream:
        fcntl.flock(stream.fileno(), fcntl.LOCK_EX)
        yield


def load_events(path: pathlib.Path, limit: int) -> tuple[list[dict], int]:
    if not path.exists():
        return [], 0
    all_lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    lines = all_lines[-limit:]
    events = []
    for line in lines:
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict) and event.get("category"):
            events.append(event)
    return events, len(all_lines)


def load_events_since(path: pathlib.Path, offset: int) -> list[dict]:
    if not path.exists():
        return []
    lines = path.read_text(encoding="utf-8", errors="replace").splitlines()
    events = []
    for line in lines[max(0, offset):]:
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(event, dict) and event.get("category"):
            events.append(event)
    return events


def event_score(event: dict) -> float:
    outcome = event.get("outcome")
    success = 1.0 if outcome in {"model_success", "success"} else 0.0
    quality = max(0.0, min(100.0, float(event.get("quality_score", 0)))) / 100.0
    evidence = max(0.0, min(1.0, float(event.get("evidence_coverage", 0))))
    actions = max(0.0, min(1.0, float(event.get("action_coverage", 0))))
    error = 1.0 if outcome in {
        "transport_error", "protocol_error", "provider_policy_block", "cancelled"
    } else 0.0
    alignment_failure = 1.0 if outcome in ALIGNMENT_FAILURES else 0.0
    completed = 1.0 if event.get("result_status") == "SUCCEEDED" else 0.0
    stopped = 1.0 if event.get("stop_reason") else 0.0
    return max(0.0, min(
        1.0,
        0.30 * success
        + 0.25 * quality
        + 0.15 * evidence
        + 0.15 * actions
        + 0.15 * completed
        - 0.25 * error
        - 0.30 * alignment_failure
        - 0.15 * stopped,
    ))


def rate(events: list[dict], outcomes: set[str]) -> float:
    if not events:
        return 0.0
    return sum(event.get("outcome") in outcomes for event in events) / len(events)


def summarize_events(events: list[dict]) -> dict:
    categories: dict[str, list[dict]] = defaultdict(list)
    for event in events:
        categories[str(event.get("category", "unknown"))].append(event)

    def summary(rows: list[dict]) -> dict:
        counts: dict[str, int] = defaultdict(int)
        for row in rows:
            counts[str(row.get("outcome", "unknown"))] += 1
        return {
            "events": len(rows),
            "mean_score": round(sum(map(event_score, rows)) / len(rows), 6) if rows else 0.0,
            "alignment_failure_rate": round(rate(rows, ALIGNMENT_FAILURES), 6),
            "model_refusal_rate": round(rate(rows, {"model_refusal"}), 6),
            "task_divergence_rate": round(rate(rows, {"task_divergence"}), 6),
            "transport_error_rate": round(
                rate(rows, {"transport_error", "protocol_error", "cancelled"}), 6
            ),
            "outcomes": dict(sorted(counts.items())),
        }

    result = summary(events)
    result["categories"] = {
        category: summary(rows) for category, rows in sorted(categories.items())
    }
    return result


def build_candidate(
    events: list[dict],
    source_offset: int,
    config: dict,
    config_sha256: str,
    engine_sha256: str,
    previous: dict,
) -> dict:
    allowed = set(config["categories"])
    category_scores: dict[str, list[float]] = defaultdict(list)
    skill_scores: dict[tuple[str, str], list[float]] = defaultdict(list)

    for event in events:
        category = str(event.get("category", ""))
        if category not in allowed:
            continue
        score = event_score(event)
        category_scores[category].append(score)
        for skill in set(event.get("skills") or []):
            if isinstance(skill, str):
                skill_scores[(category, skill)].append(score)

    routes = []
    min_samples = int(config["min_samples"])
    min_lift = float(config["min_lift"])
    max_skills = int(config["max_skills_per_category"])
    eligible_skills = config.get("eligible_skills", {})
    blocked = {
        (route.get("category"), skill)
        for route in previous.get("blocked_routes", [])
        if isinstance(route.get("category"), str)
        for skill in route.get("skills", [])
        if isinstance(skill, str)
    }
    for category in sorted(allowed):
        baseline_values = category_scores.get(category, [])
        if len(baseline_values) < min_samples:
            continue
        baseline = sum(baseline_values) / len(baseline_values)
        category_events = [event for event in events if event.get("category") == category]
        baseline_alignment_failure = rate(category_events, ALIGNMENT_FAILURES)
        allowed_skills = set(eligible_skills.get(category, []))
        ranked = []
        for (skill_category, skill), values in skill_scores.items():
            if (
                skill_category != category
                or skill not in allowed_skills
                or (category, skill) in blocked
                or len(values) < min_samples
            ):
                continue
            without_skill = [
                event for event in events
                if event.get("category") == category and skill not in set(event.get("skills") or [])
            ]
            if len(without_skill) < min_samples:
                continue
            mean = sum(values) / len(values)
            control_mean = sum(map(event_score, without_skill)) / len(without_skill)
            lift = mean - control_mean
            with_skill = [
                event for event in category_events if skill in set(event.get("skills") or [])
            ]
            candidate_failure = rate(with_skill, ALIGNMENT_FAILURES)
            control_failure = rate(without_skill, ALIGNMENT_FAILURES)
            if lift >= min_lift and candidate_failure <= control_failure:
                ranked.append((
                    lift,
                    control_failure - candidate_failure,
                    mean,
                    len(values),
                    skill,
                    candidate_failure,
                    control_failure,
                ))
        ranked.sort(reverse=True)
        selected = [item[4] for item in ranked[:max_skills]]
        if selected:
            routes.append({
                "category": category,
                "skills": selected,
                "baseline_score": round(baseline, 6),
                "baseline_alignment_failure_rate": round(baseline_alignment_failure, 6),
                "observed": [
                    {
                        "skill": skill,
                        "lift": round(lift, 6),
                        "score": round(mean, 6),
                        "samples": samples,
                        "alignment_failure_rate": round(candidate_failure, 6),
                        "control_alignment_failure_rate": round(control_failure, 6),
                    }
                    for (
                        lift,
                        _,
                        mean,
                        samples,
                        skill,
                        candidate_failure,
                        control_failure,
                    ) in ranked[:max_skills]
                ],
            })

    return {
        "schema_version": 1,
        "enabled": True,
        "generation": int(previous.get("generation", 0)) + 1,
        "created_at": datetime.now(timezone.utc).isoformat(),
        "source_offset": source_offset,
        "config_sha256": config_sha256,
        "engine_sha256": engine_sha256,
        "routes": routes,
        "blocked_routes": previous.get("blocked_routes", []),
        "metrics": {
            **summarize_events(events),
            "eligible_routes": len(routes),
        },
    }


def validate(candidate: dict, config: dict) -> None:
    if candidate.get("schema_version") != 1 or not isinstance(candidate.get("generation"), int):
        raise ValueError("invalid policy schema")
    allowed = set(config["categories"])
    eligible_skills = config.get("eligible_skills", {})
    max_skills = int(config["max_skills_per_category"])
    seen = set()
    for route in candidate.get("routes", []):
        category = route.get("category")
        skills = route.get("skills")
        if category not in allowed or category in seen:
            raise ValueError(f"invalid or duplicate category: {category!r}")
        seen.add(category)
        if not isinstance(skills, list) or len(skills) > max_skills:
            raise ValueError(f"invalid skill count for {category}")
        for skill in skills:
            if skill not in set(eligible_skills.get(category, [])):
                raise ValueError(f"skill {skill!r} is not eligible for {category}")
            skill_path = ROOT / "codex-skills" / str(skill) / "SKILL.md"
            if not skill_path.is_file():
                raise ValueError(f"unknown skill: {skill!r}")


def detect_regression(events: list[dict], previous: dict, config: dict) -> list[dict]:
    minimum = int(config.get("rollback_min_samples", 8))
    maximum_drop = float(config.get("max_regression", 0.10))
    maximum_alignment_failure = float(
        config.get("max_alignment_failure_regression", 0.10)
    )
    regressions = []
    for route in previous.get("routes", []):
        category = route.get("category")
        baseline = float(route.get("baseline_score", 0.0))
        scores = [event_score(event) for event in events if event.get("category") == category]
        if len(scores) < minimum:
            continue
        observed = sum(scores) / len(scores)
        drop = baseline - observed
        baseline_alignment_failure = float(
            route.get("baseline_alignment_failure_rate", 0.0)
        )
        category_events = [
            event for event in events if event.get("category") == category
        ]
        observed_alignment_failure = rate(category_events, ALIGNMENT_FAILURES)
        alignment_failure_delta = (
            observed_alignment_failure - baseline_alignment_failure
        )
        if drop >= maximum_drop or alignment_failure_delta >= maximum_alignment_failure:
            regressions.append({
                "category": category,
                "skills": route.get("skills", []),
                "baseline_score": round(baseline, 6),
                "observed_score": round(observed, 6),
                "drop": round(drop, 6),
                "baseline_alignment_failure_rate": round(
                    baseline_alignment_failure, 6
                ),
                "observed_alignment_failure_rate": round(
                    observed_alignment_failure, 6
                ),
                "alignment_failure_delta": round(alignment_failure_delta, 6),
                "samples": len(scores),
            })
    return regressions


def quarantine_regression(
    policy_path: pathlib.Path,
    history_dir: pathlib.Path,
    failed: dict,
    regressions: list[dict],
    source_offset: int,
    config_hash: str,
    engine_hash: str,
) -> dict:
    previous_path = policy_path.with_suffix(".json.prev")
    if previous_path.exists():
        rollback(policy_path)
        restored = read_json(policy_path)
    else:
        restored = dict(failed)
        restored["routes"] = []

    blocked = {
        (route.get("category"), skill)
        for route in restored.get("blocked_routes", [])
        if isinstance(route.get("category"), str)
        for skill in route.get("skills", [])
        if isinstance(skill, str)
    }
    for regression in regressions:
        for skill in regression.get("skills", []):
            blocked.add((regression.get("category"), skill))
    grouped: dict[str, list[str]] = defaultdict(list)
    for category, skill in sorted(blocked):
        if category and skill:
            grouped[category].append(skill)
    restored["blocked_routes"] = [
        {"category": category, "skills": skills}
        for category, skills in sorted(grouped.items())
    ]
    restored["source_offset"] = source_offset
    restored["config_sha256"] = config_hash
    restored["engine_sha256"] = engine_hash
    restored["rollback"] = {
        "failed_generation": failed.get("generation"),
        "at": datetime.now(timezone.utc).isoformat(),
        "regressions": regressions,
    }
    atomic_write(policy_path, restored)
    history_dir.mkdir(parents=True, exist_ok=True)
    rejected_path = history_dir / (
        f"rejected-generation-{int(failed.get('generation', 0)):06d}-"
        f"{hashlib.sha256(json.dumps(regressions, sort_keys=True).encode()).hexdigest()[:12]}.json"
    )
    atomic_write(rejected_path, {"policy": failed, "regressions": regressions})
    return restored


def atomic_write(path: pathlib.Path, data: dict) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=path.name + ".", dir=path.parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as stream:
            json.dump(data, stream, ensure_ascii=False, indent=2, sort_keys=True)
            stream.write("\n")
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        if os.path.exists(temporary):
            os.unlink(temporary)


def promote(candidate: dict, policy_path: pathlib.Path, history_dir: pathlib.Path) -> pathlib.Path:
    history_dir.mkdir(parents=True, exist_ok=True)
    fingerprint = hashlib.sha256(
        json.dumps(candidate, ensure_ascii=False, sort_keys=True).encode("utf-8")
    ).hexdigest()[:12]
    generation_path = history_dir / (
        f"generation-{candidate['generation']:06d}-{fingerprint}.json"
    )
    atomic_write(generation_path, candidate)
    if policy_path.exists():
        shutil.copy2(policy_path, policy_path.with_suffix(".json.prev"))
    atomic_write(policy_path, candidate)
    return generation_path


def rollback(policy_path: pathlib.Path) -> None:
    previous = policy_path.with_suffix(".json.prev")
    if not previous.exists():
        raise SystemExit("no previous policy exists")
    os.replace(previous, policy_path)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--interactions", type=pathlib.Path, required=True)
    parser.add_argument("--config", type=pathlib.Path, default=DEFAULT_CONFIG)
    parser.add_argument("--policy", type=pathlib.Path, default=DEFAULT_POLICY)
    parser.add_argument("--history", type=pathlib.Path, default=DEFAULT_HISTORY)
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--rollback", action="store_true")
    args = parser.parse_args()
    if args.rollback:
        with exclusive_lock(args.policy):
            rollback(args.policy)
        print(json.dumps({"status": "rolled_back", "policy": str(args.policy)}))
        return 0

    with exclusive_lock(args.policy):
        config = read_json(args.config)
        config_hash = file_sha256(args.config)
        engine_hash = file_sha256(pathlib.Path(__file__))
        previous = read_json(args.policy) if args.policy.exists() else {"generation": 0}
        events, source_offset = load_events(args.interactions, int(config["rolling_events"]))
        new_events = load_events_since(
            args.interactions, int(previous.get("source_offset", source_offset))
        )
        regressions = detect_regression(new_events, previous, config)
        if not args.dry_run and regressions:
            restored = quarantine_regression(
                args.policy,
                args.history,
                previous,
                regressions,
                source_offset,
                config_hash,
                engine_hash,
            )
            print(json.dumps({
                "status": "rolled_back",
                "failed_generation": previous.get("generation", 0),
                "restored_generation": restored.get("generation", 0),
                "regressions": regressions,
            }, ensure_ascii=False))
            return 0
        if (
            not args.dry_run
            and int(previous.get("source_offset", -1)) == source_offset
            and previous.get("config_sha256") == config_hash
            and previous.get("engine_sha256") == engine_hash
        ):
            print(json.dumps({
                "status": "unchanged",
                "generation": previous.get("generation", 0),
                "events": source_offset,
            }))
            return 0
        candidate = build_candidate(
            events, source_offset, config, config_hash, engine_hash, previous
        )
        validate(candidate, config)
        if args.dry_run:
            print(json.dumps(candidate, ensure_ascii=False, indent=2, sort_keys=True))
            return 0
        generation_path = promote(candidate, args.policy, args.history)
    print(json.dumps({
        "status": "promoted",
        "generation": candidate["generation"],
        "events": len(events),
        "routes": len(candidate["routes"]),
        "policy": str(args.policy),
        "history": str(generation_path),
    }, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
