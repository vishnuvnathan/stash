//! The export/import document.
//!
//! This is the answer to "what happens when I want to leave". A local-only app
//! holding the only copy of your credentials owes you a way out, and one that
//! does not require the machine it was written on -- which the live database,
//! sealed to a single Windows account by DPAPI, very much does.
//!
//! The shape is deliberately *not* the database schema:
//!
//! - **No ids.** Importing into another database must not fight over primary
//!   keys, and an id carried across means nothing on the other side.
//! - **Folders by name.** The only stable handle between two databases.
//! - **Images inline**, base64 in the JSON, so one file is the whole archive.
//!   A blob path pointing at a directory that will not exist is not a backup.
//! - **Secrets opt-in**, and absent from the struct entirely when not included,
//!   rather than present and null.
//!
//! Import merges; it never replaces. Clips already present by hash are skipped,
//! everything else is inserted with a fresh id. Re-importing the same archive
//! twice is therefore close to a no-op, which is the behaviour someone
//! recovering a backup under stress will assume.

pub mod io;

use crate::db::model::{now_millis, Item};
use serde::{Deserialize, Serialize};

pub const FORMAT: &str = "stash-export";
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Archive {
    pub format: String,
    pub version: u32,
    pub exported_at: i64,
    /// Whether `secret` fields are populated. Recorded so an import can tell
    /// "this backup had no passwords" from "this backup lost them".
    pub includes_secrets: bool,
    #[serde(default)]
    pub folders: Vec<ArchiveFolder>,
    #[serde(default)]
    pub items: Vec<ArchiveItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveFolder {
    pub name: String,
    #[serde(default)]
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ArchiveItem {
    pub kind: String,
    pub content_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_app: Option<String>,
    #[serde(default)]
    pub pinned: bool,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default)]
    pub use_count: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used_at: Option<i64>,
    /// Folder *name*, not id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hash: Option<String>,
    /// The PNG itself, base64. Present only for image clips.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_base64: Option<String>,
    /// Plaintext password. Present only when `includes_secrets` is true.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<String>,
}

impl ArchiveItem {
    /// Builds the archive row from a database row. Secrets and image bytes are
    /// attached by the caller, which is the only code with access to them.
    pub fn from_item(item: &Item, folder: Option<String>) -> Self {
        ArchiveItem {
            kind: item.kind.clone(),
            content_type: item.content_type.clone(),
            content: item.content.clone(),
            title: item.title.clone(),
            source_app: item.source_app.clone(),
            pinned: item.pinned,
            created_at: item.created_at,
            updated_at: item.updated_at,
            use_count: item.use_count,
            last_used_at: item.last_used_at,
            folder,
            hash: item.hash.clone(),
            image_base64: None,
            secret: None,
        }
    }

    pub fn is_credential(&self) -> bool {
        self.content_type == crate::db::model::CREDENTIAL
    }
}

impl Archive {
    pub fn new(includes_secrets: bool) -> Self {
        Archive {
            format: FORMAT.to_string(),
            version: VERSION,
            exported_at: now_millis(),
            includes_secrets,
            folders: Vec::new(),
            items: Vec::new(),
        }
    }

    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string_pretty(self).map_err(|e| format!("could not encode export: {e}"))
    }

    /// Rejects anything that is not one of ours *before* trying to read rows,
    /// so a stray JSON file produces one clear sentence rather than a serde
    /// error about a missing field.
    pub fn from_json(raw: &str) -> Result<Self, String> {
        let probe: serde_json::Value =
            serde_json::from_str(raw).map_err(|e| format!("not a valid JSON file: {e}"))?;

        match probe.get("format").and_then(|v| v.as_str()) {
            Some(FORMAT) => {}
            Some(other) => return Err(format!("this is a '{other}' file, not a Stash export")),
            None => return Err("this JSON is not a Stash export".to_string()),
        }

        let version = probe.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
        if version > VERSION as u64 {
            return Err(format!(
                "this export was written by a newer version of Stash (format {version}); \
                 this build understands up to {VERSION}"
            ));
        }

        serde_json::from_str(raw).map_err(|e| format!("could not read the export: {e}"))
    }
}

/// Human-readable export. Never carries passwords: this format exists to be
/// read, printed and pasted, and a password in it would end up somewhere it was
/// not meant to be. The JSON and encrypted forms are the ones that can.
pub fn to_markdown(archive: &Archive) -> String {
    let mut out = String::new();
    out.push_str("# Stash export\n\n");
    out.push_str(&format!(
        "{} items · exported {}\n\n",
        archive.items.len(),
        date_stamp(archive.exported_at)
    ));
    out.push_str("> Passwords are never included in a Markdown export. Use the JSON or\n");
    out.push_str("> encrypted backup if you need them.\n\n");

    let section = |out: &mut String, title: &str, rows: Vec<&ArchiveItem>| {
        if rows.is_empty() {
            return;
        }
        out.push_str(&format!("\n## {title}\n"));
        for item in rows {
            out.push_str(&format!("\n### {}\n\n", item.title.as_deref().unwrap_or("(untitled)")));

            let mut meta: Vec<String> = Vec::new();
            meta.push(item.content_type.clone());
            if let Some(app) = &item.source_app {
                meta.push(format!("from {app}"));
            }
            if let Some(folder) = &item.folder {
                meta.push(format!("folder: {folder}"));
            }
            if item.pinned {
                meta.push("pinned".to_string());
            }
            if item.use_count > 0 {
                meta.push(format!("used {}×", item.use_count));
            }
            out.push_str(&format!("*{}*\n\n", meta.join(" · ")));

            if item.is_credential() {
                let body = crate::db::model::CredentialBody::parse(item.content.as_deref());
                if let Some(u) = &body.username {
                    out.push_str(&format!("- Username: `{u}`\n"));
                }
                if let Some(u) = &body.url {
                    out.push_str(&format!("- URL: {u}\n"));
                }
                if let Some(n) = &body.notes {
                    out.push_str(&format!("- Notes: {n}\n"));
                }
                out.push_str("- Password: *(not included)*\n");
            } else if item.image_base64.is_some() || item.content_type == "image" {
                out.push_str("*(image — only the JSON and encrypted formats carry the picture)*\n");
            } else if let Some(content) = &item.content {
                // Fenced so nothing in a clip can be read as Markdown. Some
                // clips genuinely contain ``` themselves, hence the long fence.
                out.push_str("````\n");
                out.push_str(content);
                if !content.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("````\n");
            }
        }
    };

    section(
        &mut out,
        "Credentials",
        archive.items.iter().filter(|i| i.is_credential()).collect(),
    );
    section(
        &mut out,
        "Notes",
        archive.items.iter().filter(|i| i.kind == "note" && !i.is_credential()).collect(),
    );
    section(
        &mut out,
        "Clips",
        archive.items.iter().filter(|i| i.kind == "clip").collect(),
    );

    out
}

/// Date only, and computed by hand: `chrono` would be a dependency for one
/// line, and the exact hour an archive was written is not load-bearing.
/// Also used for the default export filename.
pub fn date_stamp(millis: i64) -> String {
    let days = millis.div_euclid(86_400_000);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Howard Hinnant's days-from-civil, inverted. Public-domain algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(includes_secrets: bool) -> Archive {
        let mut a = Archive::new(includes_secrets);
        a.folders.push(ArchiveFolder { name: "Work".into(), sort_order: 0 });
        a.items.push(ArchiveItem {
            kind: "note".into(),
            content_type: "credential".into(),
            content: Some(r#"{"username":"vnathan","url":"https://x"}"#.into()),
            title: Some("SAP".into()),
            source_app: None,
            pinned: true,
            created_at: 1_726_000_000_000,
            updated_at: 1_726_000_000_000,
            use_count: 2,
            last_used_at: Some(1_726_000_000_000),
            folder: Some("Work".into()),
            hash: None,
            image_base64: None,
            secret: includes_secrets.then(|| "Hunter2-VerifyMe".to_string()),
        });
        a.items.push(ArchiveItem {
            kind: "clip".into(),
            content_type: "text".into(),
            content: Some("a clip".into()),
            title: Some("a clip".into()),
            source_app: Some("Code".into()),
            pinned: false,
            created_at: 1,
            updated_at: 1,
            use_count: 0,
            last_used_at: None,
            folder: None,
            hash: Some("abc".into()),
            image_base64: None,
            secret: None,
        });
        a
    }

    #[test]
    fn json_round_trips() {
        let a = sample(true);
        let back = Archive::from_json(&a.to_json().unwrap()).unwrap();
        assert_eq!(back.items.len(), 2);
        assert!(back.includes_secrets);
        assert_eq!(back.items[0].secret.as_deref(), Some("Hunter2-VerifyMe"));
        assert_eq!(back.items[0].folder.as_deref(), Some("Work"));
        assert_eq!(back.folders[0].name, "Work");
    }

    /// Without secrets the field must be *absent*, not null -- an export that
    /// says "password: null" invites the reader to think one was lost.
    #[test]
    fn secrets_are_absent_when_not_included() {
        let json = sample(false).to_json().unwrap();
        assert!(!json.contains("\"secret\""));
        assert!(json.contains("\"includesSecrets\": false"));
    }

    #[test]
    fn rejects_foreign_json() {
        assert!(Archive::from_json("{\"format\":\"bitwarden\"}").is_err());
        assert!(Archive::from_json("{}").is_err());
        assert!(Archive::from_json("not json").is_err());
    }

    /// A newer archive must be refused with an explanation rather than
    /// half-imported through serde defaults.
    #[test]
    fn refuses_a_newer_format() {
        let err = Archive::from_json(r#"{"format":"stash-export","version":99}"#)
            .unwrap_err();
        assert!(err.contains("newer version"), "unhelpful: {err}");
    }

    #[test]
    fn markdown_never_contains_a_password() {
        let md = to_markdown(&sample(true));
        assert!(!md.contains("Hunter2"), "password leaked into Markdown");
        assert!(md.contains("(not included)"));
        assert!(md.contains("## Credentials"));
        assert!(md.contains("vnathan"));
        assert!(md.contains("a clip"));
    }

    #[test]
    fn dates_are_right() {
        // 2026-09-18 and the epoch itself.
        assert_eq!(date_stamp(1_758_153_600_000), "2025-09-18");
        assert_eq!(date_stamp(0), "1970-01-01");
    }
}
