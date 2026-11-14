use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MessageSummary {
    pub uid: u32,
    pub mailbox: String,
    pub from: String,
    pub to: String,
    pub subject: String,
    pub date: String,
    pub timestamp: Option<i64>,
    pub size: u32,
    pub flags: Vec<String>,
    pub unread: bool,
    pub starred: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedMessage {
    pub uid: u32,
    pub mailbox: String,
    pub from: String,
    pub to: String,
    pub cc: String,
    pub bcc: String,
    pub reply_to: String,
    pub subject: String,
    pub date: String,
    pub timestamp: Option<i64>,
    pub message_id: Option<String>,
    pub in_reply_to: Option<String>,
    pub references: Option<String>,
    pub body_text: String,
    pub body_html: Option<String>,
    pub attachments: Vec<Attachment>,
    pub flags: Vec<String>,
    pub unread: bool,
    pub starred: bool,
    pub size: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Attachment {
    pub index: usize,
    pub filename: String,
    pub mime_type: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub name: String,
    pub delimiter: Option<String>,
    pub attributes: Vec<String>,
    pub selectable: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SendResult {
    pub message_id: String,
    pub from: String,
    pub recipients: Vec<String>,
}
