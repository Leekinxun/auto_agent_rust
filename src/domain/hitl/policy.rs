use serde::{Deserialize, Serialize};

use super::models::HitlRiskLevel;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HitlDefaultAction {
    Auto,
    RequireApproval,
    Reject,
}

impl Default for HitlDefaultAction {
    fn default() -> Self {
        Self::Auto
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct HitlPolicyRule {
    pub tool: Option<String>,
    pub tool_prefix: Option<String>,
    pub display_name: Option<String>,
    pub require_approval: bool,
    pub risk_level: HitlRiskLevel,
}

impl Default for HitlPolicyRule {
    fn default() -> Self {
        Self {
            tool: None,
            tool_prefix: None,
            display_name: None,
            require_approval: false,
            risk_level: HitlRiskLevel::Medium,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HitlPolicyDecision {
    Allow,
    Reject {
        reason: String,
    },
    RequireApproval {
        risk_level: HitlRiskLevel,
        display_name: Option<String>,
    },
}

pub fn evaluate_tool_hitl_policy(
    enabled: bool,
    default_action: &HitlDefaultAction,
    rules: &[HitlPolicyRule],
    tool_name: &str,
) -> HitlPolicyDecision {
    if !enabled {
        return HitlPolicyDecision::Allow;
    }
    let normalized_tool = tool_name.trim();
    for rule in rules {
        if !rule_matches(rule, normalized_tool) {
            continue;
        }
        return if rule.require_approval {
            HitlPolicyDecision::RequireApproval {
                risk_level: rule.risk_level.clone(),
                display_name: rule
                    .display_name
                    .as_deref()
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string),
            }
        } else {
            HitlPolicyDecision::Allow
        };
    }

    match default_action {
        HitlDefaultAction::Auto => HitlPolicyDecision::Allow,
        HitlDefaultAction::RequireApproval => HitlPolicyDecision::RequireApproval {
            risk_level: HitlRiskLevel::Medium,
            display_name: None,
        },
        HitlDefaultAction::Reject => HitlPolicyDecision::Reject {
            reason: "HITL default action rejects unmatched tool calls".to_string(),
        },
    }
}

fn rule_matches(rule: &HitlPolicyRule, tool_name: &str) -> bool {
    if let Some(tool) = rule
        .tool
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && tool == tool_name
    {
        return true;
    }
    if let Some(prefix) = rule
        .tool_prefix
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        && tool_name.starts_with(prefix)
    {
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::{HitlDefaultAction, HitlPolicyDecision, HitlPolicyRule, evaluate_tool_hitl_policy};
    use crate::domain::hitl::models::HitlRiskLevel;

    #[test]
    fn matching_rule_requires_approval() {
        let decision = evaluate_tool_hitl_policy(
            true,
            &HitlDefaultAction::Auto,
            &[HitlPolicyRule {
                tool: Some("write_file".to_string()),
                require_approval: true,
                risk_level: HitlRiskLevel::High,
                ..HitlPolicyRule::default()
            }],
            "write_file",
        );
        assert_eq!(
            decision,
            HitlPolicyDecision::RequireApproval {
                risk_level: HitlRiskLevel::High,
                display_name: None
            }
        );
    }

    #[test]
    fn matching_rule_carries_display_name() {
        let decision = evaluate_tool_hitl_policy(
            true,
            &HitlDefaultAction::Auto,
            &[HitlPolicyRule {
                tool: Some("write_file".to_string()),
                display_name: Some("写入作战文档".to_string()),
                require_approval: true,
                risk_level: HitlRiskLevel::High,
                ..HitlPolicyRule::default()
            }],
            "write_file",
        );
        assert_eq!(
            decision,
            HitlPolicyDecision::RequireApproval {
                risk_level: HitlRiskLevel::High,
                display_name: Some("写入作战文档".to_string())
            }
        );
    }

    #[test]
    fn disabled_policy_allows() {
        let decision = evaluate_tool_hitl_policy(
            false,
            &HitlDefaultAction::RequireApproval,
            &[],
            "write_file",
        );
        assert_eq!(decision, HitlPolicyDecision::Allow);
    }
}
