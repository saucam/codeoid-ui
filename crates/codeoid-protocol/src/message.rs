//! Session messages — the unit of observable work. Every prompt, tool call,
//! tool result, and assistant reply flows as one of these.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

use crate::tool::ToolInfo;

/// Who produced a message. Every message carries one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IdentityType {
    Human,
    Agent,
    Subagent,
    System,
}

/// Identity of the entity that produced a message.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageIdentity {
    /// ZeroID WIMSE URI.
    pub sub: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(rename = "type")]
    pub kind: IdentityType,
}

/// Role discriminator on each [`SessionMessage`]. Simple frontends render
/// based on role alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageRole {
    User,
    Assistant,
    Thinking,
    ToolCall,
    ToolResult,
    System,
    Info,
}

/// Rich structured content. Discriminated on `kind` — unknown kinds
/// deserialize into [`ContentPart::Unknown`] and are rendered as raw JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentPart {
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        markdown: Option<bool>,
    },
    Code {
        code: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        language: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "filePath")]
        file_path: Option<String>,
    },
    FileRef {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lines: Option<[u32; 2]>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        change: Option<FileChange>,
    },
    Diff {
        path: String,
        added: u32,
        removed: u32,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            rename = "originalPath"
        )]
        original_path: Option<String>,
    },
    Tree {
        label: String,
        children: Vec<TreeNode>,
    },
    Button {
        label: String,
        action: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        data: Option<BTreeMap<String, Value>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        style: Option<ButtonStyle>,
    },
    Progress {
        message: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        percent: Option<u8>,
        #[serde(default, skip_serializing_if = "Option::is_none", rename = "elapsedMs")]
        elapsed_ms: Option<u64>,
    },
    Image {
        url: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alt: Option<String>,
    },
    Anchor {
        uri: String,
        title: String,
    },
    Table {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Forward-compat sink: new daemon-side part kinds land here with their
    /// raw JSON preserved. Frontend logs + falls back to rendering the parent
    /// message's `content` string.
    #[serde(other, deserialize_with = "deserialize_unknown")]
    Unknown,
}

fn deserialize_unknown<'de, D>(deserializer: D) -> Result<(), D::Error>
where
    D: serde::Deserializer<'de>,
{
    // `#[serde(other)]` needs a unit variant; this just satisfies the trait.
    serde::de::IgnoredAny::deserialize(deserializer).map(|_| ())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileChange {
    pub added: u32,
    pub removed: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeNode {
    pub label: String,
    #[serde(rename = "type")]
    pub kind: TreeNodeType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<TreeNode>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TreeNodeType {
    File,
    Directory,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ButtonStyle {
    Primary,
    Secondary,
    Danger,
}

/// Complete session message.
///
/// Mirrors `SessionMessage` in the TS protocol. Self-contained, auditable,
/// and always carries identity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessage {
    pub session_id: String,
    pub message_id: String,
    pub role: MessageRole,
    /// Plain text — always present, used by simple frontends.
    pub content: String,
    /// Rich parts — optional, used by capable frontends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts: Option<Vec<ContentPart>>,
    pub identity: MessageIdentity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<ToolInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<BTreeMap<String, Value>>,
    pub timestamp: String,
}

/// Incremental update applied to an existing [`SessionMessage`] by `message_id`.
///
/// * `content_append` — concatenate to `content`.
/// * `parts_append` / `parts_update` — extend or replace entries in `parts`.
/// * `tool_state_update` — **replace** `tool.state` wholesale; do NOT merge fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessageDelta {
    pub session_id: String,
    pub message_id: String,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_append: Option<String>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts_append: Option<Vec<ContentPart>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parts_update: Option<Vec<PartsUpdate>>,

    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_state_update: Option<crate::tool::ToolState>,

    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PartsUpdate {
    pub index: u32,
    pub part: ContentPart,
}
