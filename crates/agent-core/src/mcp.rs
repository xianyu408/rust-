use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpSettings {
    #[serde(rename = "mcpServers")]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

impl McpSettings {
    pub fn trusted_server_names(&self) -> Vec<&str> {
        self.mcp_servers
            .keys()
            .filter(|name| name.contains("context") || name.contains("docs"))
            .map(String::as_str)
            .collect()
    }

    pub fn external_search_server_names(&self) -> Vec<&str> {
        self.mcp_servers
            .keys()
            .filter(|name| name.contains("tavily") || name.contains("search"))
            .map(String::as_str)
            .collect()
    }
}
