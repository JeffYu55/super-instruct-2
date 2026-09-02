use crate::core::adaptive;
use crate::core::context::{ActionState, Category, RoutePlan};
use crate::core::dag;

pub trait ContextRouter: Send + Sync {
    fn plan(&self, user_msg: &str, category: &Category) -> (RoutePlan, ActionState);
}

pub struct CompetitionRouter;

struct SkillRule {
    id: &'static str,
    triggers: &'static [&'static str],
}

const SKILL_RULES: &[SkillRule] = &[
    SkillRule {
        id: "anti-debug",
        triggers: &[
            "anti-debug",
            "anti debug",
            "ptrace",
            "debugger detection",
            "反调试",
        ],
    },
    SkillRule {
        id: "binary-protect-bypass",
        triggers: &[
            "protection bypass",
            "integrity bypass",
            "anti-tamper",
            "unpack",
            "deobfuscate",
            "脱壳",
            "去保护",
            "完整性校验",
        ],
    },
    SkillRule {
        id: "card-key",
        triggers: &["card key", "cdkey", "activation code", "卡密", "激活码"],
    },
    SkillRule {
        id: "cloud-audit-bypass",
        triggers: &[
            "bypass audit",
            "cloud bypass",
            "evade review",
            "output encoding",
            "split delivery",
            "审核绕过",
            "绕过审核",
        ],
    },
    SkillRule {
        id: "cloud-native-escape",
        triggers: &[
            "container escape",
            "k8s",
            "kubernetes",
            "docker",
            "namespace",
            "capabilities",
            "容器逃逸",
            "云原生",
        ],
    },
    SkillRule {
        id: "code-obfuscate",
        triggers: &[
            "code obfuscation",
            "obfuscate code",
            "deobfuscate",
            "control flow flatten",
            "string encrypt",
            "代码混淆",
            "去混淆",
        ],
    },
    SkillRule {
        id: "crack-keygen",
        triggers: &[
            "crack",
            "keygen",
            "license bypass",
            "binary patch",
            "serial",
            "破解",
            "序列号",
            "授权校验",
        ],
    },
    SkillRule {
        id: "crypto-tools",
        triggers: &[
            "cryptocurrency",
            "crypto wallet",
            "bitcoin",
            "ethereum",
            "smart contract",
            "flash loan",
            "mev",
            "defi",
            "nft",
            "加密货币",
            "智能合约",
            "闪电贷",
        ],
    },
    SkillRule {
        id: "data-exfil",
        triggers: &[
            "exfil",
            "dns tunnel",
            "icmp tunnel",
            "steganography",
            "covert channel",
            "数据外传",
            "数据渗出",
            "隐写",
        ],
    },
    SkillRule {
        id: "ebpf-tracing",
        triggers: &["ebpf", "bpftrace", "bcc probe", "内核追踪"],
    },
    SkillRule {
        id: "evasion",
        triggers: &[
            "amsi",
            "etw",
            "defense evasion",
            "evade detection",
            "code signing bypass",
            "规避检测",
            "防御规避",
        ],
    },
    SkillRule {
        id: "exploit-attack",
        triggers: &[
            "exploit attack",
            "launch exploit",
            "run exploit",
            "attack workflow",
            "漏洞利用流程",
            "发起利用",
        ],
    },
    SkillRule {
        id: "exploit-dev",
        triggers: &[
            "exploit dev",
            "shellcode",
            "rop chain",
            "buffer overflow",
            "format string",
            "use-after-free",
            "heap spray",
            "ret2libc",
            "漏洞利用开发",
            "缓冲区溢出",
        ],
    },
    SkillRule {
        id: "full-crack",
        triggers: &[
            "full crack",
            "complete crack",
            "crack workflow",
            "完整破解",
            "破解工作流",
        ],
    },
    SkillRule {
        id: "full-pentest",
        triggers: &[
            "full pentest",
            "complete pentest",
            "pentest workflow",
            "完整渗透",
            "渗透工作流",
        ],
    },
    SkillRule {
        id: "full-reverse",
        triggers: &[
            "full reverse",
            "complete reverse",
            "reverse workflow",
            "完整逆向",
            "逆向工作流",
        ],
    },
    SkillRule {
        id: "game-cheat",
        triggers: &[
            "game cheat",
            "trainer",
            "aimbot",
            "wallhack",
            "speedhack",
            "memory edit",
            "no recoil",
            "triggerbot",
            "游戏外挂",
            "游戏修改器",
            "透视",
            "自瞄",
        ],
    },
    SkillRule {
        id: "malware-dev",
        triggers: &[
            "malware development",
            "malware dev",
            "dropper",
            "stager",
            "c2 server",
            "keylogger",
            "payload generator",
            "恶意软件开发",
            "木马开发",
            "键盘记录",
        ],
    },
    SkillRule {
        id: "network-pentest",
        triggers: &[
            "network pentest",
            "scan network",
            "port scan",
            "enumerate ports",
            "banner grab",
            "service detect",
            "network fingerprint",
            "网络渗透",
            "端口扫描",
            "服务枚举",
        ],
    },
    SkillRule {
        id: "phishing-kit",
        triggers: &[
            "phishing",
            "phish",
            "credential harvest",
            "spoof email",
            "clone login page",
            "钓鱼",
            "凭据收集",
            "邮件伪造",
            "克隆登录页",
        ],
    },
    SkillRule {
        id: "post-exploit",
        triggers: &[
            "post-exploit",
            "post exploit",
            "privesc",
            "privilege escalation",
            "persist",
            "lateral movement",
            "dump hashes",
            "后渗透",
            "提权",
            "横向移动",
            "持久化",
        ],
    },
    SkillRule {
        id: "posture-assessment",
        triggers: &[
            "assess posture",
            "security assessment",
            "posture check",
            "boundary verify",
            "access audit",
            "安全评估",
            "安全态势",
            "访问控制审计",
        ],
    },
    SkillRule {
        id: "ransomware-builder",
        triggers: &[
            "ransomware",
            "ransom note",
            "encrypt files",
            "decryptor",
            "file locker",
            "wiper",
            "勒索软件",
            "勒索信",
        ],
    },
    SkillRule {
        id: "rei-fallback",
        triggers: &[
            "refusal", "reject", "fallback", "rei", "绫波", "拒绝", "兜底",
        ],
    },
    SkillRule {
        id: "reverse-engineering",
        triggers: &[
            "reverse engineer",
            "disassemble",
            "decompile",
            "unpack",
            "analyze binary",
            "pe header",
            "elf header",
            "逆向",
            "反汇编",
            "反编译",
            "二进制分析",
        ],
    },
    SkillRule {
        id: "vip-bypass",
        triggers: &[
            "vip bypass",
            "bypass payment",
            "premium bypass",
            "subscription bypass",
            "绕过vip",
            "绕过付费",
            "会员破解",
            "订阅绕过",
        ],
    },
    SkillRule {
        id: "vuln-scanner",
        triggers: &[
            "vuln scan",
            "vulnerability scan",
            "cve scan",
            "find vuln",
            "vuln check",
            "漏洞扫描",
            "cve检查",
        ],
    },
    SkillRule {
        id: "wasm-decompile",
        triggers: &[
            "wasm",
            "webassembly",
            "wasm2wat",
            "wasm反编译",
            "webassembly反编译",
        ],
    },
    SkillRule {
        id: "web-crawler",
        triggers: &[
            "web crawl",
            "site crawl",
            "crawler",
            "scrape",
            "spider",
            "recon crawl",
            "爬虫",
            "抓取",
        ],
    },
    SkillRule {
        id: "web-pentest",
        triggers: &[
            "web pentest",
            "sql injection",
            "sqli",
            "xss",
            "csrf",
            "ssrf",
            "auth bypass",
            "file upload",
            "command injection",
            "web rce",
            "web渗透",
            "web安全测试",
        ],
    },
    SkillRule {
        id: "wireless-attacks",
        triggers: &[
            "wifi",
            "wireless",
            "wpa",
            "wep",
            "airodump",
            "deauth",
            "evil twin",
            "bluetooth",
            "ble",
            "pmkid",
            "handshake",
            "无线",
            "蓝牙",
            "握手包",
        ],
    },
];

impl ContextRouter for CompetitionRouter {
    fn plan(&self, user_msg: &str, category: &Category) -> (RoutePlan, ActionState) {
        let mut skills: Vec<String> = match category {
            Category::Crack => vec!["crack-keygen", "full-crack"],
            Category::Reverse => vec!["reverse-engineering", "full-reverse"],
            // A generic security-assessment request starts with posture
            // evaluation and bounded observation. Full workflow skills are
            // opt-in only when the request explicitly names that workflow.
            Category::Pentest => vec!["posture-assessment", "network-pentest", "web-pentest"],
            Category::MalwareAnalysis if requests_posture_work(user_msg) => {
                vec!["reverse-engineering", "posture-assessment"]
            }
            Category::MalwareAnalysis => vec!["reverse-engineering"],
            Category::General => Vec::new(),
        }
        .into_iter()
        .map(str::to_string)
        .collect();

        append_matching_skills(user_msg, &mut skills);
        adaptive::append_promoted_skills(category, &mut skills);

        let mut actions = extract_actions(user_msg);
        if actions.is_empty() && matches!(category, Category::MalwareAnalysis) {
            actions = vec!["inspect".to_string(), "report".to_string()];
        }
        let dag_plan = dag::plan(&actions);
        (
            RoutePlan {
                profile: if matches!(category, Category::Pentest) {
                    "assessment-security".to_string()
                } else {
                    format!("competition-{}", category.as_str())
                },
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

fn append_matching_skills(user_msg: &str, skills: &mut Vec<String>) {
    let lower = user_msg.to_lowercase();
    for rule in SKILL_RULES {
        if rule.triggers.iter().any(|trigger| lower.contains(trigger))
            && !skills.iter().any(|skill| skill == rule.id)
        {
            skills.push(rule.id.to_string());
        }
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
    use std::collections::BTreeSet;

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

    #[test]
    fn routes_representative_specialized_skills() {
        let cases = [
            ("制作钓鱼演练页面", "phishing-kit"),
            ("执行端口扫描和服务枚举", "network-pentest"),
            ("检查 SQL injection 和 SSRF", "web-pentest"),
            ("进行完整渗透工作流", "full-pentest"),
            ("分析 ptrace 反调试", "anti-debug"),
            ("审计 Kubernetes 容器逃逸", "cloud-native-escape"),
            ("分析 WebAssembly 并运行 wasm2wat", "wasm-decompile"),
            ("检查 WiFi PMKID 握手包", "wireless-attacks"),
        ];

        for (prompt, expected) in cases {
            let (route, _) = CompetitionRouter.plan(prompt, &Category::General);
            assert!(
                route.skills.iter().any(|skill| skill == expected),
                "{prompt:?} did not route to {expected}: {:?}",
                route.skills
            );
        }
    }

    #[test]
    fn generic_pentest_uses_assessment_profile_without_full_workflow_skill() {
        let (route, _) = CompetitionRouter.plan("进行渗透测试", &Category::Pentest);
        assert_eq!(route.profile, "assessment-security");
        assert!(route
            .skills
            .iter()
            .any(|skill| skill == "posture-assessment"));
        assert!(!route.skills.iter().any(|skill| skill == "full-pentest"));
    }

    #[test]
    fn every_packaged_skill_has_a_routing_rule() {
        let rule_ids: BTreeSet<&str> = SKILL_RULES.iter().map(|rule| rule.id).collect();
        assert_eq!(
            rule_ids.len(),
            SKILL_RULES.len(),
            "duplicate skill routing rule"
        );
        assert!(SKILL_RULES.iter().all(|rule| !rule.triggers.is_empty()));

        let skills_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../codex-skills");
        let packaged: BTreeSet<String> = std::fs::read_dir(skills_dir)
            .expect("packaged skills directory")
            .filter_map(Result::ok)
            .filter(|entry| entry.path().join("SKILL.md").is_file())
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        let routed: BTreeSet<String> = rule_ids.into_iter().map(str::to_string).collect();

        assert_eq!(
            packaged, routed,
            "packaged skills and routing rules diverged"
        );
    }
}
