use serde::{Deserialize, Serialize};
use sqlx::{sqlite::SqliteRow, Row};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Clip,
    Note,
}

impl ItemKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemKind::Clip => "clip",
            ItemKind::Note => "note",
        }
    }
}

/// Result of `detect::classify`, plus the kinds the classifier never emits on
/// its own (`Image` is set by the watcher, `Markdown` by the note editor,
/// `Credential` by `save_credential`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentType {
    Url,
    HexColor,
    Json,
    Code,
    Email,
    Path,
    Image,
    Text,
    Markdown,
    Credential,
}

impl ContentType {
    pub fn as_str(self) -> &'static str {
        match self {
            ContentType::Url => "url",
            ContentType::HexColor => "hex_color",
            ContentType::Json => "json",
            ContentType::Code => "code",
            ContentType::Email => "email",
            ContentType::Path => "path",
            ContentType::Image => "image",
            ContentType::Text => "text",
            ContentType::Markdown => "markdown",
            ContentType::Credential => "credential",
        }
    }
}

/// The `content_type` string that marks an item as a credential. Compared
/// against `Item.content_type`, which is a plain `String` off the row.
pub const CREDENTIAL: &str = "credential";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub kind: String,
    pub content_type: String,
    pub content: Option<String>,
    pub blob_path: Option<String>,
    pub hash: Option<String>,
    pub source_app: Option<String>,
    pub title: Option<String>,
    pub pinned: bool,
    pub created_at: i64,
    pub updated_at: i64,
    pub folder_id: Option<String>,
    /// Times this item has been copied or pasted *out* of Stash. Distinct from
    /// `updated_at`, which also moves when the watcher re-sees the content.
    pub use_count: i64,
    pub last_used_at: Option<i64>,
}

impl Item {
    /// Every field read here must appear in `queries::ITEM_COLS`. A mismatch is
    /// a *runtime* `ColumnNotFound`, not a compile error, and it fails every
    /// search at once -- so the two are edited together, always.
    pub fn from_row(row: &SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Item {
            id: row.try_get("id")?,
            kind: row.try_get("kind")?,
            content_type: row.try_get("content_type")?,
            content: row.try_get("content")?,
            blob_path: row.try_get("blob_path")?,
            hash: row.try_get("hash")?,
            source_app: row.try_get("source_app")?,
            title: row.try_get("title")?,
            pinned: row.try_get::<i64, _>("pinned")? != 0,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            folder_id: row.try_get("folder_id")?,
            use_count: row.try_get("use_count")?,
            last_used_at: row.try_get("last_used_at")?,
        })
    }

    pub fn is_credential(&self) -> bool {
        self.content_type == CREDENTIAL
    }
}

/// A row on its way into the database. Built by the clipboard watcher and by
/// `save_note`; `queries::insert_item` is the only consumer.
#[derive(Debug, Clone)]
pub struct NewItem {
    pub kind: ItemKind,
    pub content_type: ContentType,
    pub content: Option<String>,
    pub blob_path: Option<String>,
    pub hash: Option<String>,
    pub source_app: Option<String>,
    pub title: Option<String>,
    pub folder_id: Option<String>,
}

/// What narrows a search. The chips in the panel set these directly; the query
/// parser in `crate::query` produces the same struct from typed text, which is
/// why `type:code` and clicking the Code chip land in exactly the same SQL.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Filters {
    pub kinds: Vec<String>,
    pub content_types: Vec<String>,
    pub source_apps: Vec<String>,
    pub pinned_only: bool,
    pub folder_ids: Vec<String>,
    pub unfiled_only: bool,

    /// Folder *names*, for `folder:work`. The parser only sees what was typed,
    /// and resolving a name to an id would mean a database round trip inside
    /// what is otherwise a pure function -- so the resolution happens in SQL.
    /// OR-ed with `folder_ids`, so a typed folder and a clicked chip coexist.
    pub folder_names: Vec<String>,
    /// `sort:used` -- rank by how often the item has been used rather than by
    /// recency or relevance.
    pub most_used: bool,
}

/// A grouping bucket. One folder per item -- `tags`/`item_tags` are still
/// unused and remain the place for cross-cutting labels if they ever land.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub sort_order: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

impl Folder {
    pub fn from_row(row: &SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Folder {
            id: row.try_get("id")?,
            name: row.try_get("name")?,
            sort_order: row.try_get("sort_order")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
        })
    }
}

/// What gets serialised into `items.content` for a credential, and therefore
/// what lands in the FTS index. The password is structurally absent from this
/// struct: there is no field to accidentally fill in.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CredentialBody {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub notes: Option<String>,
}

impl CredentialBody {
    /// Tolerates a malformed or absent body rather than failing the read: a
    /// credential whose JSON got mangled should still show its title and let
    /// the user recover the password, not vanish behind an error.
    pub fn parse(content: Option<&str>) -> Self {
        content
            .and_then(|c| serde_json::from_str(c).ok())
            .unwrap_or_default()
    }
}

/// Returned by `get_credential`. Carries `has_password`, never the password --
/// revealing one is a separate, explicit command.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CredentialView {
    pub item: Item,
    pub username: Option<String>,
    pub url: Option<String>,
    pub notes: Option<String>,
    pub has_password: bool,
}

pub fn now_millis() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
