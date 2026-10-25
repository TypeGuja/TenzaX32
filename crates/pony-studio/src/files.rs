//! Бэкенд встроенного проводника (окно рисует веб-интерфейс, см.
//! ui/dialogs.js): содержимое папки, быстрые места, недавние папки,
//! создание папки. Перенесено из egui-проводника `pony-gui`.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::{json, Value};

const MAX_RECENT: usize = 10;

fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC") => PathBuf::from(rest),
        _ => p,
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")).map(PathBuf::from).filter(|p| p.is_dir())
}

/// Быстрые места: что реально существует на этой машине.
fn places() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    if let Ok(cwd) = std::env::current_dir() {
        out.push(("project".to_string(), cwd));
    }
    if let Some(home) = home_dir() {
        out.push(("home".to_string(), home.clone()));
        for (label, names) in [("desktop", ["Desktop", "Рабочий стол"]), ("documents", ["Documents", "Документы"]), ("downloads", ["Downloads", "Загрузки"])] {
            if let Some(p) = names.iter().map(|n| home.join(n)).find(|p| p.is_dir()) {
                out.push((label.to_string(), p));
            }
        }
    }
    if cfg!(windows) {
        for letter in b'A'..=b'Z' {
            let p = PathBuf::from(format!("{}:\\", letter as char));
            if p.is_dir() {
                out.push((format!("drive:{}:", letter as char), p));
            }
        }
    } else {
        out.push(("drive:/".to_string(), PathBuf::from("/")));
    }
    out
}

fn recent_file() -> Option<PathBuf> {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| home_dir().map(|h| h.join(".config"))))?;
    Some(base.join("pony-engine").join("recent_dirs.txt"))
}

pub fn load_recent() -> Vec<PathBuf> {
    recent_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|s| s.lines().map(PathBuf::from).filter(|p| p.is_dir()).take(MAX_RECENT).collect())
        .unwrap_or_default()
}

pub fn remember(dir: &Path) {
    if cfg!(test) {
        return;
    }
    let mut recent = load_recent();
    recent.retain(|d| d != dir);
    recent.insert(0, dir.to_path_buf());
    recent.truncate(MAX_RECENT);
    if let Some(f) = recent_file() {
        if let Some(parent) = f.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let text: String = recent.iter().map(|d| format!("{}\n", d.display())).collect();
        let _ = std::fs::write(f, text);
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["Б", "КБ", "МБ", "ГБ"];
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} {}", UNITS[0])
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

/// "ГГГГ-ММ-ДД ЧЧ:ММ" (UTC) без внешних зависимостей.
pub fn format_time(t: SystemTime) -> String {
    let Ok(d) = t.duration_since(SystemTime::UNIX_EPOCH) else { return String::new() };
    let secs = d.as_secs() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + if month <= 2 { 1 } else { 0 };
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", rem / 3600, rem % 3600 / 60)
}

/// С какой папки открыть проводник: папка `start` (или папка файла
/// `start`), иначе последняя недавняя, иначе текущая.
pub fn start_dir(start: Option<&str>) -> PathBuf {
    start
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .and_then(|p| if p.is_dir() { Some(p) } else { p.parent().filter(|d| !d.as_os_str().is_empty()).map(Path::to_path_buf) })
        .filter(|d| d.is_dir())
        .or_else(|| load_recent().into_iter().next())
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Содержимое папки для окна проводника.
pub fn list(dir: &Path) -> Value {
    let dir = std::fs::canonicalize(dir).map(strip_verbatim).unwrap_or_else(|_| dir.to_path_buf());
    let base = json!({
        "dir": dir.display().to_string(),
        "parent": dir.parent().map(|p| p.display().to_string()),
        "sep": std::path::MAIN_SEPARATOR.to_string(),
        "crumbs": dir.ancestors().collect::<Vec<_>>().into_iter().rev().map(|p| json!({
            "name": p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| p.display().to_string()),
            "path": p.display().to_string(),
        })).collect::<Vec<_>>(),
        "places": places().into_iter().map(|(label, p)| json!({"label": label, "path": p.display().to_string()})).collect::<Vec<_>>(),
        "recent": load_recent().into_iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
    });
    let mut out = base;
    match std::fs::read_dir(&dir) {
        Ok(rd) => {
            let mut entries: Vec<(bool, String, Value)> = rd
                .filter_map(|e| e.ok())
                .map(|e| {
                    let path = e.path();
                    let meta = std::fs::metadata(&path).ok();
                    let is_dir = meta.as_ref().is_some_and(|m| m.is_dir());
                    let name = e.file_name().to_string_lossy().into_owned();
                    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
                    let entry = json!({
                        "name": name,
                        "path": path.display().to_string(),
                        "dir": is_dir,
                        "xfl": is_dir && path.join("DOMDocument.xml").is_file(),
                        "size": if is_dir { String::new() } else { human_size(size) },
                        "mtime": meta.and_then(|m| m.modified().ok()).map(format_time).unwrap_or_default(),
                    });
                    (is_dir, name.to_lowercase(), entry)
                })
                .collect();
            entries.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
            out["entries"] = Value::Array(entries.into_iter().map(|e| e.2).collect());
        }
        Err(e) => {
            out["entries"] = json!([]);
            out["error"] = json!(format!("Не удалось открыть {}: {e}", dir.display()));
        }
    }
    out
}

pub fn stat(path: &str) -> Value {
    let p = Path::new(path);
    json!({
        "exists": p.exists(),
        "dir": p.is_dir(),
        "file": p.is_file(),
        "xfl": p.is_dir() && p.join("DOMDocument.xml").is_file(),
        "parent_ok": p.parent().is_some_and(|d| d.as_os_str().is_empty() || d.is_dir()),
        "absolute": p.is_absolute(),
    })
}

pub fn join(dir: &str, name: &str) -> String {
    if Path::new(name).is_absolute() {
        name.to_string()
    } else {
        Path::new(dir).join(name).display().to_string()
    }
}

pub fn mkdir(dir: &str, name: &str) -> Result<String, String> {
    let name = name.trim();
    if name.is_empty() || name.contains(['/', '\\']) {
        return Err("Недопустимое имя папки".into());
    }
    let p = Path::new(dir).join(name);
    std::fs::create_dir(&p).map_err(|e| format!("Не удалось создать {}: {e}", p.display()))?;
    Ok(p.display().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_and_size_formatting() {
        assert_eq!(format_time(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000)), "2023-11-14 22:13");
        assert_eq!(human_size(512), "512 Б");
        assert_eq!(human_size(1536), "1.5 КБ");
    }

    #[test]
    fn list_puts_folders_first_and_marks_xfl_folders() {
        let dir = std::env::temp_dir().join(format!("pony_files_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("zz_folder")).unwrap();
        std::fs::create_dir_all(dir.join("anim")).unwrap();
        std::fs::write(dir.join("anim").join("DOMDocument.xml"), "<x/>").unwrap();
        std::fs::write(dir.join("a.asset"), "x").unwrap();
        let v = list(&dir);
        let names: Vec<&str> = v["entries"].as_array().unwrap().iter().map(|e| e["name"].as_str().unwrap()).collect();
        assert_eq!(names, ["anim", "zz_folder", "a.asset"]);
        assert_eq!(v["entries"][0]["xfl"], true);
        assert!(mkdir(&dir.display().to_string(), "../bad").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }
}
