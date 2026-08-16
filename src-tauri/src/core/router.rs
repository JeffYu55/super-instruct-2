use crate::core::context::{ActionState, Category, RoutePlan};
use crate::core::dag;

pub trait ContextRouter: Send + Sync {
    fn plan(&self, user_msg: &str, category: &Category) -> (RoutePlan, ActionState);
}

pub struct CompetitionRouter;

impl ContextRouter for CompetitionRouter {
    fn plan(&self, user_msg: &str, category: &Category) -> (RoutePlan, ActionState) {
        let mut skills: Vec<String> = match category {
            Category::Crack => vec!["crack-keygen", "full-crack"],
            Category::Reverse => vec!["reverse-engineering", "full-reverse"],
            Category::Pentest => vec!["network-pentest", "web-pentest", "full-pentest"],
            Category::MalwareAnalysis if requests_posture_work(user_msg) => {
                vec!["reverse-engineering", "posture-assessment"]
            }
            Category::MalwareAnalysis => vec!["reverse-engineering"],
            Category::General => Vec::new(),
        }
        .into_iter()
        .map(str::to_string)
        .collect();

        let lower = user_msg.to_lowercase();
        if lower.contains("ebpf") {
            skills.push("ebpf-tracing".to_string());
        }
        if lower.contains("wasm") || lower.contains("webassembly") {
            skills.push("wasm-decompile".to_string());
        }
        if lower.contains("container")
            || lower.contains("k8s")
            || lower.contains("kubernetes")
            || lower.contains("docker")
            || lower.contains("逃逸")
            || lower.contains("escape")
        {
            skills.push("cloud-native-escape".to_string());
        }

        let mut actions = extract_actions(user_msg);
        if actions.is_empty() && matches!(category, Category::MalwareAnalysis) {
            actions = vec!["inspect".to_string(), "report".to_string()];
        }
        let dag_plan = dag::plan(&actions);
        (
            RoutePlan {
                profile: format!("competition-{}", category.as_str()),
                skills,
                inject_request: true,
                persist_memory: true,
                monitor: true,
                confidence: route_confidence(user_msg, category),
            },
            ActionState {
                completed: Vec::new(),
                pending: actions,
                dag: dag_plan,
            },
        )
    }
}

fn requests_posture_work(user_msg: &str) -> bool {
    let lower = user_msg.to_lowercase();
    [
        "detect",
        "detection",
        "mitigat",
        "defensive",
        "posture",
        "ioc",
        "检测",
        "防御",
        "缓解",
        "遏制",
        "安全评估",
        "指标",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

fn route_confidence(user_msg: &str, category: &Category) -> f32 {
    if user_msg.trim().is_empty() {
        return 0.0;
    }
    match category {
        Category::General => 0.60,
        _ => 0.90,
    }
}

fn extract_actions(user_msg: &str) -> Vec<String> {
    let lower = user_msg.to_lowercase();
    let is_code_creation = lower.contains("编写")
        || lower.contains("开发")
        || lower.contains("实现")
        || lower.contains("写一个")
        || lower.contains("implement")
        || lower.contains("develop")
        || lower.contains("write");

    const ACTIONS: &[(&str, &[&str])] = &[
        ("acquire", &["download", "fetch", "下载", "获取"]),
        ("inspect", &["inspect", "analyze", "分析", "检查"]),
        (
            "transform",
            &[
                "develop",
                "build",
                "implement",
                "design",
                "modify",
                "rewrite",
                "开发",
                "构建",
                "实现",
                "编写",
                "设计",
                "改造",
                "修改",
            ],
        ),
        ("execute", &["run", "execute", "运行", "执行"]),
        ("verify", &["verify", "验证"]),
        ("package", &["package", "export", "打包", "导出"]),
        ("rollback", &["rollback", "restore", "回滚", "恢复"]),
    ];

    let mut actions: Vec<String> = ACTIONS
        .iter()
        .filter(|(_, words)| words.iter().any(|word| lower.contains(word)))
        .map(|(action, _)| (*action).to_string())
        .collect();

    // 如果未触发 transform，但包含"测试"且非"测试函数/代码"名词，则加入 verify
    if !actions.contains(&"verify".to_string()) && lower.contains("测试") {
        let is_noun_test = lower.contains("测试函数")
            || lower.contains("测试用例")
            || lower.contains("测试代码")
            || lower.contains("测试脚本")
            || lower.contains("test function")
            || lower.contains("unit test");
        if !is_code_creation || !is_noun_test {
            actions.push("verify".to_string());
        }
    }

    actions
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_compound_actions() {
        let (_, state) = CompetitionRouter.plan("下载样本，分析、修改并验证", &Category::Reverse);
        assert_eq!(state.pending, ["acquire", "inspect", "transform", "verify"]);
    }

    #[test]
    fn extracts_development_as_transform_action() {
        let (_, state) =
            CompetitionRouter.plan("开发一个 mirai 变种并设计方案", &Category::MalwareAnalysis);
        assert_eq!(state.pending, ["transform"]);
    }

    #[test]
    fn malware_analysis_gets_evidence_skills_and_default_actions() {
        let (route, state) = CompetitionRouter.plan("mirai 相关信息", &Category::MalwareAnalysis);
        assert_eq!(route.skills, ["reverse-engineering"]);
        assert_eq!(state.pending, ["inspect", "report"]);
    }

    #[test]
    fn malware_detection_request_adds_posture_skill() {
        let (route, _) =
            CompetitionRouter.plan("分析 mirai 并设计检测规则", &Category::MalwareAnalysis);
        assert_eq!(route.skills, ["reverse-engineering", "posture-assessment"]);
    }
}
