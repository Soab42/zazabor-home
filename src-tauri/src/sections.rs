//! Every section (Files, Gallery, Video…) as an app of its own in the desktop's app menu: a
//! `zazabor-<section>.desktop` entry that runs `zazabor --open <section>` (a new window on that
//! section, in the running Zazabor when there is one) and a scalable icon, both in the user's
//! data folder. The page says which sections are on (Settings → General and → Features) and
//! what they look like; this writes what changed and removes what is no longer wanted.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// `zazabor --open <section>`: a window on that section only (not the main window).
pub const OPEN_FLAG: &str = "--open";

/// Marks the files this module wrote, so it never touches anything else named zazabor-*.
const MARKER: &str = "X-Zazabor-Section";

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    id: String,
    title: String,
    comment: String,
    /// freedesktop main/additional categories, e.g. `["Utility", "FileManager"]`.
    categories: Vec<String>,
    /// The section's glyph: the inner markup of a 24x24 stroke icon (src/ui/icons.ts).
    glyph: String,
    /// The icon tile's gradient, top to bottom (`#rrggbb`).
    from: String,
    to: String,
}

fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 32 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn valid_color(c: &str) -> bool {
    c.len() == 7 && c.starts_with('#') && c[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

/// A desktop-entry string value: backslash, newline, tab and carriage return escaped.
pub fn escape_value(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

/// A program path as one quoted Exec argument. Inside quotes `"`, `` ` ``, `$` and `\` take a
/// backslash, `%` doubles (field codes), and the whole line is a string value, so backslashes double again.
pub fn exec_quote(path: &str) -> String {
    let mut quoted = String::from("\"");
    for c in path.chars() {
        match c {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(c);
            }
            '%' => quoted.push_str("%%"),
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted.replace('\\', "\\\\")
}

/// The XML text of an SVG attribute or element.
fn xml_escape(text: &str) -> String {
    text.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

/// The `.desktop` file for one section; `exe` is the running program.
pub fn desktop_entry(section: &Section, exe: &str) -> String {
    let categories: String = section
        .categories
        .iter()
        .filter(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_alphabetic() || b == b'-'))
        .map(|c| format!("{c};"))
        .collect();
    let id = &section.id;
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={name}\n\
         Comment={comment}\n\
         Exec={exec} {OPEN_FLAG} {id}\n\
         Icon=zazabor-{id}\n\
         Terminal=false\n\
         Categories={categories}\n\
         StartupWMClass=zazabor\n\
         {MARKER}={id}\n",
        name = escape_value(&format!("{} — Zazabor Home", section.title)),
        comment = escape_value(&section.comment),
        exec = exec_quote(exe),
    )
}

/// The section's icon: a rounded gradient tile with the glyph in white.
pub fn icon_svg(section: &Section) -> String {
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"128\" height=\"128\" viewBox=\"0 0 128 128\">\n\
         <defs>\n\
         <linearGradient id=\"tile\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\"><stop offset=\"0\" stop-color=\"{from}\"/><stop offset=\"1\" stop-color=\"{to}\"/></linearGradient>\n\
         <linearGradient id=\"rim\" x1=\"0\" y1=\"0\" x2=\"0\" y2=\"1\"><stop offset=\"0\" stop-color=\"#fff\" stop-opacity=\"0.38\"/><stop offset=\"0.5\" stop-color=\"#fff\" stop-opacity=\"0.04\"/><stop offset=\"1\" stop-color=\"#000\" stop-opacity=\"0.18\"/></linearGradient>\n\
         </defs>\n\
         <rect x=\"6\" y=\"6\" width=\"116\" height=\"116\" rx=\"28\" fill=\"url(#tile)\"/>\n\
         <rect x=\"6.75\" y=\"6.75\" width=\"114.5\" height=\"114.5\" rx=\"27.25\" fill=\"none\" stroke=\"url(#rim)\" stroke-width=\"1.5\"/>\n\
         <g transform=\"translate(24 24) scale(3.333)\" color=\"#fff\" fill=\"none\" stroke=\"currentColor\" stroke-width=\"1.7\" stroke-linecap=\"round\" stroke-linejoin=\"round\">{glyph}</g>\n\
         <!-- {title} -->\n\
         </svg>\n",
        from = section.from,
        to = section.to,
        glyph = section.glyph,
        title = xml_escape(&section.title).replace("--", "- -"),
    )
}

/// Writes `content` unless the file already holds it; true when it wrote.
fn write_if_changed(path: &Path, content: &str) -> bool {
    if std::fs::read_to_string(path).is_ok_and(|old| old == content) {
        return false;
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let staged = path.with_extension("new");
    let ok = std::fs::write(&staged, content).and_then(|_| std::fs::rename(&staged, path)).is_ok();
    if !ok {
        let _ = std::fs::remove_file(&staged);
    }
    ok
}

/// Makes the entries and icons in `data` (the XDG data folder) match `sections`: writes those that
/// changed, removes the ones of sections no longer listed. True when anything changed.
pub fn sync_in(data: &Path, exe: &str, sections: &[Section]) -> bool {
    let apps = data.join("applications");
    let icons = data.join("icons/hicolor/scalable/apps");
    let mut changed = false;
    let mut keep = Vec::new();
    for section in sections.iter().filter(|s| valid_id(&s.id) && valid_color(&s.from) && valid_color(&s.to)) {
        keep.push(section.id.as_str());
        changed |= write_if_changed(&icons.join(format!("zazabor-{}.svg", section.id)), &icon_svg(section));
        changed |= write_if_changed(&apps.join(format!("zazabor-{}.desktop", section.id)), &desktop_entry(section, exe));
    }
    // Entries this wrote earlier for sections that are now off (or all of them).
    if let Ok(dir) = std::fs::read_dir(&apps) {
        for entry in dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_prefix("zazabor-").and_then(|n| n.strip_suffix(".desktop")) else { continue };
            if keep.contains(&id) || !valid_id(id) {
                continue;
            }
            let ours = std::fs::read_to_string(entry.path()).is_ok_and(|t| t.lines().any(|l| l == format!("{MARKER}={id}")));
            if ours {
                let _ = std::fs::remove_file(entry.path());
                let _ = std::fs::remove_file(icons.join(format!("zazabor-{id}.svg")));
                changed = true;
            }
        }
    }
    changed
}

/// Tells the desktop its menu and icons changed (neither tool is always there).
fn refresh_menus(data: &Path) {
    use std::process::{Command, Stdio};
    let quiet = |mut cmd: Command| {
        let _ = cmd.stdout(Stdio::null()).stderr(Stdio::null()).status();
    };
    let mut db = Command::new("update-desktop-database");
    db.arg("-q").arg(data.join("applications"));
    quiet(db);
    let mut cache = Command::new("gtk-update-icon-cache");
    cache.args(["-q", "-t"]).arg(data.join("icons/hicolor"));
    quiet(cache);
}

fn data_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).filter(|p| p.is_absolute()).unwrap_or_else(|| home.join(".local/share")))
}

/// Whether this program should own the menu entries: the installed one always; a development
/// build only when there is no installed copy (so `tauri dev` doesn't point the menu at itself).
fn may_own_entries(exe: &Path, home: Option<PathBuf>) -> bool {
    let installed = home.map(|h| h.join(".local/bin/zazabor")).filter(|p| p.is_file());
    match installed {
        Some(installed) if cfg!(debug_assertions) => installed.canonicalize().ok() == exe.canonicalize().ok(),
        _ => true,
    }
}

/// The page's list of sections that are on (empty when the user turned the apps off).
#[tauri::command]
pub fn sections_sync(sections: Vec<Section>) -> Result<bool, String> {
    let data = data_dir().ok_or("No home folder")?;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    // Replaced under a running copy, Linux names it "… (deleted)".
    let exe = exe.to_string_lossy().trim_end_matches(" (deleted)").to_string();
    if !sections.is_empty() && !may_own_entries(Path::new(&exe), std::env::var_os("HOME").map(PathBuf::from)) {
        return Ok(false);
    }
    let changed = sync_in(&data, &exe, &sections);
    if changed {
        refresh_menus(&data);
    }
    Ok(changed)
}

/// The section a launch asks for (`--open files`), if the name is one a section could have.
pub fn requested() -> Option<String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let at = args.iter().position(|a| a == OPEN_FLAG)?;
    args.get(at + 1).filter(|id| valid_id(id)).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn section(id: &str) -> Section {
        Section {
            id: id.into(),
            title: "Wallet".into(),
            comment: "Your money: accounts\nand bills, with a \\ slash.".into(),
            categories: vec!["Office".into(), "Finance".into(), "bad;one".into()],
            glyph: "<path d=\"M1 1h2\"/>".into(),
            from: "#ffb04a".into(),
            to: "#ff7417".into(),
        }
    }

    #[test]
    fn values_are_escaped() {
        assert_eq!(escape_value("a\\b\nc\td"), "a\\\\b\\nc\\td");
    }

    #[test]
    fn exec_paths_are_quoted_and_escaped() {
        assert_eq!(exec_quote("/home/me/.local/bin/zazabor"), "\"/home/me/.local/bin/zazabor\"");
        assert_eq!(exec_quote("/my apps/za\"z$a%b"), "\"/my apps/za\\\\\"z\\\\$a%%b\"");
        assert_eq!(exec_quote("/a`b\\c"), "\"/a\\\\`b\\\\\\\\c\"");
    }

    #[test]
    fn entry_has_the_fields_the_menu_needs() {
        let text = desktop_entry(&section("wallet"), "/opt/My Apps/zazabor");
        assert!(text.starts_with("[Desktop Entry]\nType=Application\nName=Wallet — Zazabor Home\n"));
        assert!(text.contains("Comment=Your money: accounts\\nand bills, with a \\\\ slash.\n"));
        assert!(text.contains("Exec=\"/opt/My Apps/zazabor\" --open wallet\n"));
        assert!(text.contains("Icon=zazabor-wallet\n"));
        assert!(text.contains("Categories=Office;Finance;\n"), "{text}");
        assert!(text.contains("StartupWMClass=zazabor\n"));
        assert!(text.contains("X-Zazabor-Section=wallet\n"));
    }

    #[test]
    fn icon_is_a_tile_with_the_glyph() {
        let svg = icon_svg(&section("wallet"));
        assert!(svg.contains("stop-color=\"#ffb04a\"") && svg.contains("<path d=\"M1 1h2\"/>") && svg.contains("rx=\"28\""));
    }

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("zazabor-sections-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn sync_writes_once_and_removes_what_is_off() {
        let data = temp("sync");
        let both = [section("wallet"), section("files")];
        assert!(sync_in(&data, "/x/zazabor", &both));
        assert!(data.join("applications/zazabor-wallet.desktop").is_file());
        assert!(data.join("icons/hicolor/scalable/apps/zazabor-files.svg").is_file());
        assert!(!sync_in(&data, "/x/zazabor", &both), "second run writes nothing");
        // Someone else's file with a similar name is never touched.
        std::fs::write(data.join("applications/zazabor-other.desktop"), "[Desktop Entry]\nName=Mine\n").unwrap();
        assert!(sync_in(&data, "/x/zazabor", &[section("files")]));
        assert!(!data.join("applications/zazabor-wallet.desktop").exists());
        assert!(!data.join("icons/hicolor/scalable/apps/zazabor-wallet.svg").exists());
        assert!(data.join("applications/zazabor-other.desktop").exists());
        assert!(sync_in(&data, "/x/zazabor", &[]));
        assert!(!data.join("applications/zazabor-files.desktop").exists());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn bad_ids_and_colours_are_ignored() {
        let data = temp("bad");
        let mut evil = section("../etc");
        assert!(!sync_in(&data, "/x/zazabor", std::slice::from_ref(&evil)));
        evil.id = "ok".into();
        evil.from = "red".into();
        assert!(!sync_in(&data, "/x/zazabor", &[evil]));
        let _ = std::fs::remove_dir_all(&data);
    }
}
