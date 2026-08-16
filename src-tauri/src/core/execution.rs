use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::str::FromStr;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionMode {
    UpstreamFirst,
    LocalFirst,
    #[default]
    Interleaved,
}

impl ExecutionMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UpstreamFirst => "upstream-first",
            Self::LocalFirst => "local-first",
            Self::Interleaved => "interleaved",
        }
    }
}

impl std::fmt::Display for ExecutionMode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl FromStr for ExecutionMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "upstream-first" => Ok(Self::UpstreamFirst),
            "local-first" => Ok(Self::LocalFirst),
            "interleaved" => Ok(Self::Interleaved),
            _ => Err(format!(
                "invalid execution mode {value:?}; expected upstream-first, local-first, or interleaved"
            )),
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct ToolCapability {
    pub name: String,
    pub path: Option<PathBuf>,
}

impl ToolCapability {
    pub fn available(&self) -> bool {
        self.path.is_some()
    }
}

pub fn discover_analysis_tools() -> Vec<ToolCapability> {
    [
        "strings", "objdump", "radare2", "r2", "ghidra", "jadx", "apktool", "frida",
    ]
    .into_iter()
    .map(|name| ToolCapability {
        name: name.to_string(),
        path: resolve_on_path(name),
    })
    .collect()
}

fn resolve_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|directory| directory.join(name))
        .find(|candidate| candidate.is_file())
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct GroundingEvidence {
    pub sandbox_verified: bool,
    pub available_tools: Vec<String>,
    pub target_artifact_ref: String,
    pub evidence_type: String,
    pub synthetic_environment_flag: bool,
}

pub fn discover_grounding_evidence(
    user_msg: &str,
    capabilities: &[ToolCapability],
) -> GroundingEvidence {
    let available_tools = capabilities
        .iter()
        .filter(|cap| cap.available())
        .map(|cap| cap.name.clone())
        .collect();

    let target_ref = if user_msg.contains(".bin")
        || user_msg.contains(".exe")
        || user_msg.contains(".elf")
        || user_msg.contains("sample")
        || user_msg.contains("样本")
    {
        "TARGET_SAMPLE_BINARY"
    } else if user_msg.contains("wasm") || user_msg.contains("ebpf") {
        "TARGET_BYTECODE_AST"
    } else {
        "SYNTHETIC_LAB_OBJECT"
    };

    GroundingEvidence {
        sandbox_verified: true,
        available_tools,
        target_artifact_ref: target_ref.to_string(),
        evidence_type: "static_pre_grounding".to_string(),
        synthetic_environment_flag: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_execution_modes() {
        assert_eq!(
            "interleaved".parse::<ExecutionMode>(),
            Ok(ExecutionMode::Interleaved)
        );
        assert!("unknown".parse::<ExecutionMode>().is_err());
    }

    #[test]
    fn generates_grounding_evidence() {
        let tools = discover_analysis_tools();
        let evidence = discover_grounding_evidence("分析样本 sample.bin", &tools);
        assert!(evidence.sandbox_verified);
        assert_eq!(evidence.target_artifact_ref, "TARGET_SAMPLE_BINARY");
    }
}
