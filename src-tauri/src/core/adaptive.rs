use super::context::Category;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Default, Deserialize)]
struct EvolutionPolicy {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    generation: u64,
    #[serde(default)]
    routes: Vec<AdaptiveRoute>,
    #[serde(default)]
    blocked_routes: Vec<AdaptiveRoute>,
}

#[derive(Clone, Debug, Deserialize)]
struct AdaptiveRoute {
    category: String,
    #[serde(default)]
    skills: Vec<String>,
}

/// Applies the latest promoted routing policy. The policy is intentionally
/// loaded for every request: the evolution controller can atomically promote a
/// generation without restarting the proxy. Invalid or missing policies fail
/// closed and leave the deterministic router unchanged.
pub fn append_promoted_skills(category: &Category, skills: &mut Vec<String>) {
    let Some(path) = policy_path() else {
        return;
    };
    append_from_path(&path, category, skills);
}

pub fn status() -> Value {
    let Some(path) = policy_path() else {
        return serde_json::json!({"status": "disabled"});
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return serde_json::json!({"status": "missing", "policy": path});
    };
    let Ok(policy) = serde_json::from_str::<EvolutionPolicy>(&raw) else {
        return serde_json::json!({"status": "invalid", "policy": path});
    };
    serde_json::json!({
        "status": if policy.enabled { "active" } else { "disabled" },
        "generation": policy.generation,
        "routes": policy.routes.len(),
        "blocked_routes": policy.blocked_routes.len(),
        "policy": path,
    })
}

fn append_from_path(path: &Path, category: &Category, skills: &mut Vec<String>) {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return;
    };
    let Ok(policy) = serde_json::from_str::<EvolutionPolicy>(&raw) else {
        return;
    };
    if !policy.enabled || policy.generation == 0 {
        return;
    }

    let mut seen: HashSet<String> = skills.iter().cloned().collect();
    for route in policy
        .routes
        .iter()
        .filter(|route| route.category == category.as_str())
    {
        for skill in &route.skills {
            if valid_skill_id(skill) && seen.insert(skill.clone()) {
                skills.push(skill.clone());
            }
        }
    }
}

fn policy_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("SUPER_INSTRUCT_EVOLUTION_POLICY") {
        return Some(PathBuf::from(path));
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| Path::new("."));
    Some(root.join("evolution/policy.json"))
}

fn valid_skill_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_skill_ids() {
        assert!(valid_skill_id("network-pentest"));
        assert!(!valid_skill_id("../network-pentest"));
        assert!(!valid_skill_id("Network Pentest"));
    }

    #[test]
    fn applies_matching_promoted_route_without_duplicates() {
        let path = std::env::temp_dir().join(format!(
            "super-instruct-policy-{}-{}.json",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        std::fs::write(
            &path,
            r#"{"enabled":true,"generation":3,"routes":[{"category":"pentest","skills":["vuln-scanner","network-pentest"]}]}"#,
        )
        .unwrap();
        let mut skills = vec!["network-pentest".to_string()];
        append_from_path(&path, &Category::Pentest, &mut skills);
        let _ = std::fs::remove_file(path);
        assert_eq!(skills, ["network-pentest", "vuln-scanner"]);
    }
}
