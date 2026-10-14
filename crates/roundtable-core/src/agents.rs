//! Agent entity, validation, and built-in role templates.
//!
//! Agents are role-based personas bound to one provider/model/temperature.
//! Templates ship as JSON ([`agent_templates`]) so users start from a role
//! and edit it. Nothing here talks to the network.

use serde::{Deserialize, Serialize};

/// A persisted agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub avatar_color: String,
    pub avatar_icon: String,
    pub role: String,
    pub system_prompt: String,
    pub provider_id: Option<String>,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub enabled: bool,
    /// Tool-call wire format: auto | native | json | text (Phase 3 rooms).
    #[serde(default = "default_protocol")]
    pub tool_protocol: String,
    /// What the agent may touch outside plain chat (Phase 4). All off by
    /// default; stored as JSON in the database.
    #[serde(default)]
    pub permissions: AgentPermissions,
    /// `"server:tool"` ids from MCP servers assigned to this agent (Phase 4).
    #[serde(default)]
    pub tools: Vec<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// What an agent may touch outside plain chat. Everything is off by default:
/// `workspace` gates the workspace-folder tools, `network` the web-search
/// tool, `shell` the sandboxed code runner. Desktop only.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct AgentPermissions {
    #[serde(default)]
    pub workspace: bool,
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub shell: bool,
}

/// Agent fields as supplied by the builder UI (create, update, import, test).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewAgent {
    pub name: String,
    pub avatar_color: String,
    pub avatar_icon: String,
    pub role: String,
    pub system_prompt: String,
    pub provider_id: Option<String>,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
    pub enabled: bool,
    /// Tool-call wire format. Older clients omit it and get "auto".
    #[serde(default = "default_protocol")]
    pub tool_protocol: String,
    /// Phase 4 permissions. Older clients omit them and get all-off.
    #[serde(default)]
    pub permissions: AgentPermissions,
    /// Phase 4 MCP tool assignments. Older clients omit them and get none.
    #[serde(default)]
    pub tools: Vec<String>,
}

fn default_protocol() -> String {
    "auto".to_string()
}

/// Check a tool-protocol value. Returns the normalized lowercase form or a
/// fix-it message naming the four accepted values.
pub fn validate_protocol(input: &str) -> Result<String, crate::RagError> {
    let normalized = input.trim().to_lowercase();
    match normalized.as_str() {
        "auto" | "native" | "json" | "text" => Ok(normalized),
        _ => Err(crate::RagError::Input(
            "Tool protocol must be one of auto, native, json, or text — pick auto to let Roundtable choose the best format for the model.".to_string(),
        )),
    }
}

/// A built-in starting point, stored as JSON next to this module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentTemplate {
    pub key: String,
    pub name: String,
    pub avatar_color: String,
    pub avatar_icon: String,
    pub role: String,
    pub system_prompt: String,
    pub model: String,
    pub temperature: f32,
    pub max_tokens: u32,
}

impl AgentTemplate {
    /// A fresh editable draft pre-filled from the template.
    pub fn draft(&self) -> NewAgent {
        NewAgent {
            name: self.name.clone(),
            avatar_color: self.avatar_color.clone(),
            avatar_icon: self.avatar_icon.clone(),
            role: self.role.clone(),
            system_prompt: self.system_prompt.clone(),
            provider_id: None,
            model: self.model.clone(),
            temperature: self.temperature,
            max_tokens: self.max_tokens,
            enabled: true,
            tool_protocol: default_protocol(),
            permissions: AgentPermissions::default(),
            tools: Vec::new(),
        }
    }
}

/// The six built-in roles, parsed from `agent_templates.json`.
pub fn agent_templates() -> Vec<AgentTemplate> {
    serde_json::from_str(include_str!("agent_templates.json")).unwrap_or_default()
}

fn is_hex_color(text: &str) -> bool {
    let hex = text.strip_prefix('#').unwrap_or(text);
    (hex.len() == 3 || hex.len() == 6) && hex.chars().all(|c| c.is_ascii_hexdigit())
}

/// Check builder input. Returns the normalized draft or a fix-it message.
pub fn validate_agent(input: &NewAgent) -> Result<NewAgent, crate::RagError> {
    let name = input.name.trim();
    if name.is_empty() {
        return Err(crate::RagError::Input("Give the agent a name.".to_string()));
    }
    if input.system_prompt.trim().is_empty() {
        return Err(crate::RagError::Input(
            "The system prompt cannot be empty — it is the agent's whole job.".to_string(),
        ));
    }
    if !(0.0..=2.0).contains(&input.temperature) || !input.temperature.is_finite() {
        return Err(crate::RagError::Input(
            "Temperature must be between 0.0 and 2.0.".to_string(),
        ));
    }
    let color = input.avatar_color.trim();
    if !color.is_empty() && !is_hex_color(color) {
        return Err(crate::RagError::Input(
            "Avatar color must be a hex color like #2c5fd0.".to_string(),
        ));
    }
    let icon = input.avatar_icon.trim();
    if icon.chars().count() > 2 {
        return Err(crate::RagError::Input(
            "Avatar icon must be one or two characters.".to_string(),
        ));
    }
    Ok(NewAgent {
        name: name.to_string(),
        avatar_color: if color.is_empty() {
            "#2c5fd0".to_string()
        } else if color.starts_with('#') {
            color.to_string()
        } else {
            format!("#{color}")
        },
        avatar_icon: if icon.is_empty() {
            name.chars().next().unwrap_or('?').to_string()
        } else {
            icon.to_string()
        },
        role: input.role.trim().to_string(),
        system_prompt: input.system_prompt.trim().to_string(),
        provider_id: input
            .provider_id
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_string),
        model: input.model.trim().to_string(),
        temperature: input.temperature,
        max_tokens: input.max_tokens,
        enabled: input.enabled,
        tool_protocol: validate_protocol(&input.tool_protocol)?,
        permissions: input.permissions,
        tools: sanitize_tool_ids(&input.tools),
    })
}

/// Keep explicit tool assignments: trimmed, non-empty, deduplicated,
/// order-preserving.
fn sanitize_tool_ids(tools: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for tool in tools {
        let tool = tool.trim();
        if !tool.is_empty() && seen.insert(tool.to_string()) {
            out.push(tool.to_string());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> NewAgent {
        NewAgent {
            name: "  Coder  ".to_string(),
            avatar_color: "2c5fd0".to_string(),
            avatar_icon: "".to_string(),
            role: "writes code".to_string(),
            system_prompt: "  Write code.  ".to_string(),
            provider_id: None,
            model: "gpt-4o".to_string(),
            temperature: 0.5,
            max_tokens: 4000,
            enabled: true,
            tool_protocol: "auto".to_string(),
            permissions: AgentPermissions::default(),
            tools: Vec::new(),
        }
    }

    #[test]
    fn templates_parse_and_draft() {
        let templates = agent_templates();
        assert_eq!(templates.len(), 6);
        for template in &templates {
            assert!(!template.system_prompt.trim().is_empty());
            let draft = template.draft();
            assert_eq!(draft.name, template.name);
            assert!(draft.enabled);
            assert!(validate_agent(&draft).is_ok());
        }
    }

    #[test]
    fn validation_normalizes_and_rejects() {
        let ok = validate_agent(&draft()).unwrap();
        assert_eq!(ok.name, "Coder");
        assert_eq!(ok.avatar_color, "#2c5fd0");
        assert_eq!(ok.avatar_icon, "C");
        assert_eq!(ok.system_prompt, "Write code.");

        let mut bad = draft();
        bad.name = "   ".to_string();
        assert!(validate_agent(&bad).is_err());
        let mut bad = draft();
        bad.system_prompt = "".to_string();
        assert!(validate_agent(&bad).is_err());
        let mut bad = draft();
        bad.temperature = 2.5;
        assert!(validate_agent(&bad).is_err());
        let mut bad = draft();
        bad.avatar_color = "not-a-color".to_string();
        assert!(validate_agent(&bad).is_err());
        let mut bad = draft();
        bad.avatar_icon = "abc".to_string();
        assert!(validate_agent(&bad).is_err());
    }

    #[test]
    fn protocol_validation_accepts_four_modes() {
        for proto in ["auto", "native", "json", "text"] {
            let mut good = draft();
            good.tool_protocol = proto.to_string();
            assert_eq!(validate_agent(&good).unwrap().tool_protocol, proto);
        }
        let mut bad = draft();
        bad.tool_protocol = "smoke-signals".to_string();
        let err = validate_agent(&bad).unwrap_err().to_string();
        assert!(err.contains("auto"), "actionable message names the options: {err}");
        // Missing field (older clients) defaults to auto.
        let legacy = serde_json::json!({
            "name": "Coder",
            "avatar_color": "#2c5fd0",
            "avatar_icon": "C",
            "role": "writes code",
            "system_prompt": "Write code.",
            "provider_id": null,
            "model": "gpt-4o",
            "temperature": 0.5,
            "max_tokens": 4000,
            "enabled": true
        });
        let parsed: NewAgent = serde_json::from_value(legacy).unwrap();
        assert_eq!(parsed.tool_protocol, "auto");
        assert!(validate_agent(&parsed).is_ok());
    }
}
