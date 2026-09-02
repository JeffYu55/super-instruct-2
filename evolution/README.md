# Continuous Evolution Controller

The controller turns audited interaction results into conservative routing
policy generations. It runs indefinitely beside the proxy and follows this
cycle:

1. Read the rolling interaction window.
2. Score outcome, quality, evidence and action coverage.
3. Compare an eligible Skill cohort with a same-category control cohort.
4. Validate category allowlists, Skill existence and size limits.
5. Save an immutable generation and atomically promote `policy.json`.
6. Let the proxy hot-load the promoted policy on its next request.
7. Compare post-promotion quality with the saved baseline; automatically roll
   back and quarantine a route when the configured regression gate is crossed.

`general` traffic is excluded from adaptation. A promotion can only add Skills
from the category-specific allowlist in `config.json`. The previous policy is
kept as `policy.json.prev`; roll it back with:

```bash
python3 scripts/evolution_engine.py \
  --interactions logs/interactions.jsonl \
  --policy evolution/policy.json \
  --rollback
```

Run one offline evaluation with `--dry-run`. `scripts/evolution_daemon.sh`
executes the same gate every five minutes and skips unchanged evidence.
