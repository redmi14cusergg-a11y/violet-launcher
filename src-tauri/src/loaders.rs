//! Модлоадери: Fabric, Quilt (профіль JSON із мета-серверів), Forge і NeoForge (офіційний інсталятор).
//! Також розв'язання `inheritsFrom` — об'єднання version.json завантажувача з ванільним.

use crate::{data_dir, ensure_game_files, emit_progress, get_json, install_minecraft_version_blocking, safe_component, Progress};
use serde::Serialize;
use serde_json::{json, Value};
use std::{fs, path::{Path, PathBuf}, process::Command};

fn safe_ver(s: &str) -> bool {
    !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+'))
}

pub fn loader_title(loader: &str) -> &'static str {
    match loader { "fabric" => "Fabric", "quilt" => "Quilt", "forge" => "Forge", "neoforge" => "NeoForge", _ => "Vanilla" }
}

/// Ідентифікатор встановленої версії (назва папки у `versions/`).
pub fn version_id(mc: &str, loader: &str, lv: &str) -> String {
    match loader {
        "fabric" => format!("fabric-loader-{lv}-{mc}"),
        "quilt" => format!("quilt-loader-{lv}-{mc}"),
        "forge" => format!("{mc}-forge-{lv}"),
        "neoforge" => format!("neoforge-{lv}"),
        _ => mc.to_string(),
    }
}

#[tauri::command]
pub fn loader_version_id(mc_version: String, loader: String, loader_version: String) -> String {
    version_id(&mc_version, &loader, &loader_version)
}

// ---------- inheritsFrom ----------

fn lib_key(lib: &Value) -> String {
    let name = lib.get("name").and_then(Value::as_str).unwrap_or("");
    let mut p = name.split(':');
    let (g, a) = (p.next().unwrap_or(""), p.next().unwrap_or(""));
    let _version = p.next();
    let rest: Vec<&str> = p.collect(); // classifier
    format!("{g}:{a}:{}", rest.join(":"))
}

fn merge(parent: Value, child: Value) -> Value {
    let mut out = parent.clone();
    let (Some(o), Some(c)) = (out.as_object_mut(), child.as_object()) else { return child };
    for (k, v) in c {
        match k.as_str() {
            "libraries" => {
                let mut libs: Vec<Value> = v.as_array().cloned().unwrap_or_default();
                let seen: Vec<String> = libs.iter().map(lib_key).collect();
                for l in parent.get("libraries").and_then(Value::as_array).into_iter().flatten() {
                    if !seen.contains(&lib_key(l)) { libs.push(l.clone()); }
                }
                o.insert(k.clone(), Value::Array(libs));
            }
            "arguments" => {
                let mut args = parent.get("arguments").cloned().unwrap_or_else(|| json!({}));
                for part in ["jvm", "game"] {
                    let mut list: Vec<Value> = args.get(part).and_then(Value::as_array).cloned().unwrap_or_default();
                    list.extend(v.get(part).and_then(Value::as_array).cloned().unwrap_or_default());
                    args[part] = Value::Array(list);
                }
                o.insert(k.clone(), args);
            }
            "inheritsFrom" => {}
            _ => { o.insert(k.clone(), v.clone()); }
        }
    }
    out
}

/// Читає `versions/<id>/<id>.json` і рекурсивно підтягує батьківські версії.
/// Повертає об'єднаний JSON і id кореневої (ванільної) версії, чий JAR потрібен у classpath.
pub fn resolve_version(base: &Path, id: &str) -> Result<(Value, String), String> {
    fn load(base: &Path, id: &str) -> Result<Value, String> {
        if !safe_component(id) { return Err("Некоректний ідентифікатор версії".into()); }
        let p = base.join("versions").join(id).join(format!("{id}.json"));
        serde_json::from_slice(&fs::read(&p).map_err(|_| format!("Версію {id} не встановлено."))?)
            .map_err(|e| format!("Пошкоджений {id}.json: {e}"))
    }
    let mut chain = vec![(id.to_string(), load(base, id)?)];
    while let Some(parent) = chain.last().and_then(|(_, j)| j.get("inheritsFrom").and_then(Value::as_str)).map(str::to_string) {
        if chain.len() > 5 { return Err("Забагато рівнів inheritsFrom".into()); }
        let pj = load(base, &parent)?;
        chain.push((parent, pj));
    }
    let root_id = chain.last().map(|(i, _)| i.clone()).unwrap_or_default();
    let mut merged = chain.pop().map(|(_, j)| j).unwrap_or(Value::Null);
    while let Some((_, child)) = chain.pop() { merged = merge(merged, child); }
    Ok((merged, root_id))
}

// ---------- встановлені версії ----------

#[derive(Serialize)]
pub struct InstalledVersion { pub id: String, pub mc_version: String, pub loader: String }

pub fn detect_loader(id: &str) -> &'static str {
    if id.starts_with("fabric-loader-") { "fabric" } else if id.starts_with("quilt-loader-") { "quilt" }
    else if id.starts_with("neoforge-") { "neoforge" } else if id.contains("-forge-") || id.starts_with("forge-") { "forge" } else { "vanilla" }
}

#[tauri::command]
pub fn installed_versions(game_dir: String) -> Vec<InstalledVersion> {
    let dir = PathBuf::from(game_dir).join("versions");
    let mut out: Vec<InstalledVersion> = fs::read_dir(dir).into_iter().flatten().flatten().filter_map(|e| {
        let id = e.file_name().to_string_lossy().to_string();
        let j: Value = serde_json::from_slice(&fs::read(e.path().join(format!("{id}.json"))).ok()?).ok()?;
        let mc = j.get("inheritsFrom").and_then(Value::as_str).unwrap_or(&id).to_string();
        Some(InstalledVersion { loader: detect_loader(&id).into(), id, mc_version: mc })
    }).collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

// ---------- список версій завантажувача ----------

fn neoforge_prefix(mc: &str) -> Option<String> {
    let mut it = mc.split('.');
    if it.next()? != "1" { return None; }
    let minor: u32 = it.next()?.parse().ok()?;
    let patch: u32 = it.next().map(|p| p.parse().ok()).unwrap_or(Some(0))?;
    if minor < 20 || (minor == 20 && patch < 2) { return None; }
    Some(format!("{minor}.{patch}."))
}

fn list_blocking(loader: &str, mc: &str) -> Result<Vec<String>, String> {
    if !safe_ver(mc) { return Err("Некоректна версія гри".into()); }
    match loader {
        "fabric" | "quilt" => {
            let url = if loader == "fabric" { format!("https://meta.fabricmc.net/v2/versions/loader/{mc}") }
                      else { format!("https://meta.quiltmc.org/v3/versions/loader/{mc}") };
            let data = get_json(&url)?;
            let v: Vec<String> = data.as_array().into_iter().flatten()
                .filter_map(|x| x.pointer("/loader/version").and_then(Value::as_str).map(str::to_string)).take(30).collect();
            if v.is_empty() { Err(format!("{} не підтримує Minecraft {mc}.", loader_title(loader))) } else { Ok(v) }
        }
        "forge" => {
            let data = get_json("https://files.minecraftforge.net/net/minecraftforge/forge/promotions_slim.json")?;
            let promos = data.get("promos").ok_or("Некоректна відповідь Forge")?;
            let mut v: Vec<String> = Vec::new();
            for kind in ["recommended", "latest"] {
                if let Some(x) = promos.get(format!("{mc}-{kind}")).and_then(Value::as_str) {
                    if !v.iter().any(|y| y == x) { v.push(x.to_string()); }
                }
            }
            if v.is_empty() { Err(format!("Для Minecraft {mc} немає збірки Forge.")) } else { Ok(v) }
        }
        "neoforge" => {
            let prefix = neoforge_prefix(mc).ok_or("NeoForge підтримує Minecraft 1.20.2 і новіші.")?;
            let data = get_json("https://maven.neoforged.net/api/maven/versions/releases/net/neoforged/neoforge")?;
            let mut v: Vec<String> = data.get("versions").and_then(Value::as_array).into_iter().flatten()
                .filter_map(Value::as_str).filter(|s| s.starts_with(&prefix)).map(str::to_string).collect();
            v.reverse();
            v.truncate(30);
            if v.is_empty() { Err(format!("Для Minecraft {mc} немає збірки NeoForge.")) } else { Ok(v) }
        }
        _ => Ok(vec![]),
    }
}

#[tauri::command]
pub async fn loader_versions(loader: String, mc_version: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || list_blocking(&loader, &mc_version))
        .await.map_err(|e| format!("Фонова задача перервалася: {e}"))?
}

// ---------- встановлення ----------

fn run_installer(url: &str, name: &str, game_dir: &Path, java: &str, progress: Progress, title: &str) -> Result<(), String> {
    let inst_dir = data_dir().join("installers");
    fs::create_dir_all(&inst_dir).map_err(|e| e.to_string())?;
    let jar = inst_dir.join(name);
    progress(0, 0, &format!("Завантажую інсталятор {title}…"));
    crate::download_to(url, &jar)?;
    let lp = game_dir.join("launcher_profiles.json");
    if !lp.exists() { fs::write(&lp, r#"{"profiles":{},"version":3}"#).map_err(|e| e.to_string())?; }
    progress(0, 0, &format!("Встановлюю {title} (це може тривати кілька хвилин)…"));
    let mut cmd = Command::new(java);
    cmd.arg("-jar").arg(&jar).arg("--installClient").arg(game_dir).current_dir(&inst_dir);
    #[cfg(windows)]
    { use std::os::windows::process::CommandExt; cmd.creation_flags(0x08000000); }
    let out = cmd.output().map_err(|e| format!("Не вдалося запустити Java ({java}): {e}. Для {title} потрібна Java 17+."))?;
    if !out.status.success() {
        let text = format!("{}\n{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        let tail: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
        let tail = tail[tail.len().saturating_sub(8)..].join("\n");
        return Err(format!("Інсталятор {title} завершився з помилкою:\n{tail}"));
    }
    Ok(())
}

#[derive(Serialize)]
pub struct LoaderInstall { version_id: String, message: String }

pub fn install_loader_blocking(mc: String, loader: String, lv: String, game_dir: String, java_path: String, progress: Progress) -> Result<LoaderInstall, String> {
    if !safe_ver(&mc) || !safe_ver(&lv) { return Err("Некоректна версія".into()); }
    let base = PathBuf::from(&game_dir);
    let title = loader_title(&loader);
    // 1) ванільна версія
    if !base.join("versions").join(&mc).join(format!("{mc}.jar")).is_file() || !base.join("versions").join(&mc).join(format!("{mc}.json")).is_file() {
        install_minecraft_version_blocking(mc.clone(), game_dir.clone(), progress)?;
    }
    let id = version_id(&mc, &loader, &lv);
    // 2) завантажувач
    match loader.as_str() {
        "fabric" | "quilt" => {
            let url = if loader == "fabric" { format!("https://meta.fabricmc.net/v2/versions/loader/{mc}/{lv}/profile/json") }
                      else { format!("https://meta.quiltmc.org/v3/versions/loader/{mc}/{lv}/profile/json") };
            let profile = get_json(&url)?;
            let pid = profile.get("id").and_then(Value::as_str).unwrap_or(&id).to_string();
            if pid != id { return Err(format!("Несподіваний ID профілю: {pid}")); }
            let dir = base.join("versions").join(&id);
            fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            fs::write(dir.join(format!("{id}.json")), serde_json::to_vec_pretty(&profile).map_err(|e| e.to_string())?).map_err(|e| e.to_string())?;
        }
        "forge" | "neoforge" => {
            let java = if java_path.trim().is_empty() {
                let saved = crate::load_java_path();
                if saved.trim().is_empty() { "java".to_string() } else { saved }
            } else { java_path };
            let (url, name) = if loader == "forge" {
                (format!("https://maven.minecraftforge.net/net/minecraftforge/forge/{mc}-{lv}/forge-{mc}-{lv}-installer.jar"), format!("forge-{mc}-{lv}-installer.jar"))
            } else {
                (format!("https://maven.neoforged.net/releases/net/neoforged/neoforge/{lv}/neoforge-{lv}-installer.jar"), format!("neoforge-{lv}-installer.jar"))
            };
            if !base.join("versions").join(&id).join(format!("{id}.json")).is_file() {
                fs::create_dir_all(&base).map_err(|e| e.to_string())?;
                run_installer(&url, &name, &base, &java, progress, title)?;
            }
            if !base.join("versions").join(&id).join(format!("{id}.json")).is_file() {
                return Err(format!("Інсталятор {title} завершився, але версію {id} не створено."));
            }
        }
        _ => return Err("Невідомий завантажувач".into()),
    }
    // 3) бібліотеки та ресурси
    let (merged, _) = resolve_version(&base, &id)?;
    ensure_game_files(&merged, &base, progress)?;
    Ok(LoaderInstall { message: format!("{title} {lv} для Minecraft {mc} встановлено."), version_id: id })
}

#[tauri::command]
pub async fn install_with_loader(app: tauri::AppHandle, mc_version: String, loader: String, loader_version: String, game_dir: String, java_path: String) -> Result<LoaderInstall, String> {
    tauri::async_runtime::spawn_blocking(move || install_loader_blocking(mc_version, loader, loader_version, game_dir, java_path, &|d, t, l| emit_progress(&app, d, t, l)))
        .await.map_err(|e| format!("Фонова задача перервалася: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn ids() {
        assert_eq!(version_id("1.21.4", "fabric", "0.16.9"), "fabric-loader-0.16.9-1.21.4");
        assert_eq!(version_id("1.20.1", "forge", "47.3.0"), "1.20.1-forge-47.3.0");
        assert_eq!(version_id("1.21.4", "neoforge", "21.4.100"), "neoforge-21.4.100");
        assert_eq!(detect_loader("1.20.1-forge-47.3.0"), "forge");
        assert_eq!(neoforge_prefix("1.21.4").as_deref(), Some("21.4."));
        assert_eq!(neoforge_prefix("1.21").as_deref(), Some("21.0."));
        assert!(neoforge_prefix("1.19.2").is_none());
    }
    #[test] fn merge_works() {
        let p = json!({"mainClass":"A","libraries":[{"name":"x:y:1"},{"name":"asm:asm:9"}],"arguments":{"jvm":["-a"],"game":["--p"]},"assetIndex":{"id":"1"}});
        let c = json!({"mainClass":"B","inheritsFrom":"p","libraries":[{"name":"asm:asm:10"}],"arguments":{"jvm":["-b"],"game":["--c"]}});
        let m = merge(p, c);
        assert_eq!(m["mainClass"], "B");
        assert_eq!(m["libraries"].as_array().unwrap().len(), 2);
        assert_eq!(m["libraries"][0]["name"], "asm:asm:10");
        assert_eq!(m["arguments"]["jvm"], json!(["-a", "-b"]));
        assert_eq!(m["arguments"]["game"], json!(["--p", "--c"]));
        assert_eq!(m["assetIndex"]["id"], "1");
        assert!(m.get("inheritsFrom").is_none());
    }
}
