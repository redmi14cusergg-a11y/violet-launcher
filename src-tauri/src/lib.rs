mod offline_auth;
mod launch;
mod loaders;
mod profiles;
mod skins;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{atomic::{AtomicUsize, Ordering}, Mutex};
use tauri::Emitter;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::Command,
};

const MANIFEST_URL: &str = "https://piston-meta.mojang.com/mc/game/version_manifest_v2.json";
const USER_AGENT: &str = "VioletLauncher/0.2.0 (Minecraft launcher)";

fn http() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .build().map_err(|e| e.to_string())
}
fn get_json(url: &str) -> Result<Value, String> {
    http()?.get(url).send().map_err(|e| format!("Помилка мережі: {e}"))?
        .error_for_status().map_err(|e| format!("HTTP помилка: {e}"))?
        .json::<Value>().map_err(|e| format!("Не вдалося прочитати JSON: {e}"))
}
fn safe_component(s: &str) -> bool {
    !s.is_empty() && !s.contains("..") && !s.contains('/') && !s.contains('\\')
}
fn download_with(client: &reqwest::blocking::Client, url: &str, path: &Path) -> Result<(), String> {
    if path.exists() { return Ok(()); }
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    let mut response = client.get(url).send().map_err(|e| format!("Не вдалося завантажити {url}: {e}"))?
        .error_for_status().map_err(|e| format!("HTTP помилка під час завантаження: {e}"))?;
    // Пишемо у тимчасовий файл і перейменовуємо — щоб обрив не залишав напівфайлів.
    let tmp = path.with_extension("part");
    let mut file = fs::File::create(&tmp).map_err(|e| e.to_string())?;
    std::io::copy(&mut response, &mut file).map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}
fn download_to(url: &str, path: &Path) -> Result<(), String> {
    if path.exists() { return Ok(()); }
    download_with(&http()?, url, path)
}
fn allowed_for_windows(lib: &Value) -> bool {
    let Some(rules) = lib.get("rules").and_then(Value::as_array) else { return true; };
    let mut allowed = false;
    for rule in rules {
        let os = rule.get("os").and_then(|x| x.get("name")).and_then(Value::as_str);
        let matches = os.map(|name| name == "windows").unwrap_or(true);
        if matches {
            allowed = rule.get("action").and_then(Value::as_str) == Some("allow");
        }
    }
    allowed
}
fn rel_library_path(lib: &Value) -> Option<String> {
    if let Some(p) = lib.pointer("/downloads/artifact/path").and_then(Value::as_str) {
        return Some(p.to_string());
    }
    let name = lib.get("name")?.as_str()?;
    let mut parts = name.splitn(3, ':');
    let group = parts.next()?.replace('.', "/");
    let artifact = parts.next()?;
    let version = parts.next()?.split('@').next()?;
    Some(format!("{group}/{artifact}/{version}/{artifact}-{version}.jar"))
}
/// URL завантаження бібліотеки: `downloads.artifact.url` або (Fabric/Quilt) maven-база `url` + шлях за назвою.
pub(crate) fn lib_download_url(lib: &Value) -> Option<String> {
    if let Some(u) = lib.pointer("/downloads/artifact/url").and_then(Value::as_str) { if !u.is_empty() { return Some(u.to_string()); } }
    let base = lib.get("url").and_then(Value::as_str).filter(|u| !u.is_empty())?;
    Some(format!("{}/{}", base.trim_end_matches('/'), rel_library_path(lib)?))
}
#[tauri::command]
fn default_game_dir() -> String {
    if cfg!(target_os = "windows") {
        dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join(".minecraft").to_string_lossy().to_string()
    } else {
        dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")).join(".minecraft").to_string_lossy().to_string()
    }
}

#[derive(Serialize)]
struct VersionInfo { id: String, kind: String, release_time: String }
#[tauri::command]
async fn list_minecraft_versions() -> Result<Vec<VersionInfo>, String> {
    tauri::async_runtime::spawn_blocking(move || list_minecraft_versions_blocking())
        .await.map_err(|e| format!("Фонова задача перервалася: {e}"))?
}
fn list_minecraft_versions_blocking() -> Result<Vec<VersionInfo>, String> {
    let manifest = get_json(MANIFEST_URL)?;
    let versions = manifest.get("versions").and_then(Value::as_array).ok_or("Некоректний маніфест Minecraft")?;
    Ok(versions.iter().filter_map(|v| Some(VersionInfo {
        id: v.get("id")?.as_str()?.to_string(),
        kind: v.get("type")?.as_str()?.to_string(),
        release_time: v.get("releaseTime")?.as_str()?.to_string(),
    })).collect())
}

#[derive(Serialize)]
struct InstallResult { version: String, directory: String, files_downloaded: usize, message: String }

pub(crate) type Progress<'a> = &'a (dyn Fn(usize, usize, &str) + Sync);

/// Докачує бібліотеки та ресурси (assets), яких бракує або які пошкоджені (8 потоків) і
/// повідомляє про прогрес. Наявні файли пропускає, тож без втрат працює і без мережі.
pub(crate) fn ensure_game_files(version_json: &Value, base: &Path, progress: Progress) -> Result<usize, String> {
    struct Dl { url: String, path: PathBuf, label: String }
    let mut tasks: Vec<Dl> = Vec::new();
    if let Some(libs) = version_json.get("libraries").and_then(Value::as_array) {
        for lib in libs {
            if !allowed_for_windows(lib) { continue; }
            if let Some(url) = lib_download_url(lib) {
                if let Some(rel) = rel_library_path(lib) {
                    let path = base.join("libraries").join(&rel);
                    if !path.exists() { tasks.push(Dl { url, path, label: format!("Бібліотека {rel}") }); }
                }
            }
        }
    }
    if let Some(index) = version_json.get("assetIndex") {
        let index_url = index.get("url").and_then(Value::as_str).ok_or("Немає URL індексу ресурсів")?;
        let index_id = index.get("id").and_then(Value::as_str).unwrap_or("unknown");
        let index_path = base.join("assets").join("indexes").join(format!("{index_id}.json"));
        if !index_path.exists() { progress(0, 1, "Індекс ресурсів"); download_to(index_url, &index_path)?; }
        let index_data: Value = serde_json::from_slice(&fs::read(&index_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        if let Some(objects) = index_data.get("objects").and_then(Value::as_object) {
            for (name, obj) in objects {
                let Some(hash) = obj.get("hash").and_then(Value::as_str) else { continue; };
                if hash.len() < 2 { continue; }
                let path = base.join("assets").join("objects").join(&hash[..2]).join(hash);
                if let (Some(size), Ok(meta)) = (obj.get("size").and_then(Value::as_u64), fs::metadata(&path)) {
                    if meta.len() != size { let _ = fs::remove_file(&path); } // пошкоджений/неповний файл
                }
                if !path.exists() {
                    tasks.push(Dl { url: format!("https://resources.download.minecraft.net/{}/{}", &hash[..2], hash), path, label: format!("Ресурс {name}") });
                }
            }
        }
    }
    let total = tasks.len();
    if total == 0 { return Ok(0); }
    progress(0, total, "Підготовка…");
    let client = http()?;
    let next = AtomicUsize::new(0);
    let done = AtomicUsize::new(0);
    let error: Mutex<Option<String>> = Mutex::new(None);
    std::thread::scope(|sc| {
        for _ in 0..8.min(total) {
            sc.spawn(|| loop {
                if error.lock().unwrap().is_some() { break; }
                let i = next.fetch_add(1, Ordering::SeqCst);
                if i >= total { break; }
                let t = &tasks[i];
                match download_with(&client, &t.url, &t.path) {
                    Ok(()) => { let d = done.fetch_add(1, Ordering::SeqCst) + 1; progress(d, total, &t.label); }
                    Err(e) => { *error.lock().unwrap() = Some(e); break; }
                }
            });
        }
    });
    if let Some(e) = error.into_inner().unwrap() { return Err(e); }
    Ok(total)
}

/// Надсилає прогрес у фронтенд (подія `download-progress`), не частіше ніж кожен 6-й файл.
pub(crate) fn emit_progress(app: &tauri::AppHandle, done: usize, total: usize, label: &str) {
    if done <= 3 || done == total || done % 6 == 0 {
        let _ = app.emit("download-progress", json!({"done": done, "total": total, "label": label}));
    }
}

fn install_minecraft_version_blocking(version: String, game_dir: String, progress: Progress) -> Result<InstallResult, String> {
    if !safe_component(&version) { return Err("Некоректний ідентифікатор версії".into()); }
    let manifest = get_json(MANIFEST_URL)?;
    let entry = manifest.get("versions").and_then(Value::as_array)
        .and_then(|xs| xs.iter().find(|v| v.get("id").and_then(Value::as_str) == Some(version.as_str())))
        .ok_or("Версію не знайдено в офіційному маніфесті")?;
    let version_url = entry.get("url").and_then(Value::as_str).ok_or("У маніфесті немає URL версії")?;
    let version_json = get_json(version_url)?;
    let base = PathBuf::from(&game_dir);
    let versions_dir = base.join("versions").join(&version);
    fs::create_dir_all(&versions_dir).map_err(|e| e.to_string())?;
    let mut count = 0usize;
    let json_path = versions_dir.join(format!("{version}.json"));
    if !json_path.exists() {
        fs::write(&json_path, serde_json::to_vec_pretty(&version_json).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        count += 1;
    }
    let client_url = version_json.pointer("/downloads/client/url").and_then(Value::as_str)
        .ok_or("Для цієї версії немає клієнтського JAR")?;
    let client_path = versions_dir.join(format!("{version}.jar"));
    if !client_path.exists() { download_to(client_url, &client_path)?; count += 1; }

    count += ensure_game_files(&version_json, &base, progress)?;
    Ok(InstallResult {
        version: version.clone(), directory: base.to_string_lossy().to_string(), files_downloaded: count,
        message: format!("Файли версії {version} завантажено. Для повного запуску ще потрібні авторизація та формування аргументів запуску.")
    })
}

#[tauri::command]
async fn install_minecraft_version(app: tauri::AppHandle, version: String, game_dir: String) -> Result<InstallResult, String> {
    tauri::async_runtime::spawn_blocking(move || install_minecraft_version_blocking(version, game_dir, &|d, t, l| emit_progress(&app, d, t, l)))
        .await
        .map_err(|e| format!("Задача завантаження перервалася: {e}"))?
}

#[tauri::command]
async fn open_official_launcher(game_dir: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Ok(pf) = std::env::var("ProgramFiles") {
            candidates.push(PathBuf::from(&pf).join("Minecraft Launcher").join("MinecraftLauncher.exe"));
        }
        if let Ok(pf86) = std::env::var("ProgramFiles(x86)") {
            candidates.push(PathBuf::from(&pf86).join("Minecraft Launcher").join("MinecraftLauncher.exe"));
        }
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            candidates.push(PathBuf::from(&local).join("Programs").join("Minecraft Launcher").join("MinecraftLauncher.exe"));
        }
        let launcher = candidates.into_iter().find(|p| p.is_file())
            .ok_or_else(|| "Не знайшов Minecraft Launcher. Встанови офіційний лаунчер із minecraft.net і спробуй ще раз.".to_string())?;
        Command::new(&launcher).arg("--workDir").arg(&game_dir).spawn()
            .map_err(|e| format!("Не вдалося відкрити Minecraft Launcher: {e}"))?;
        Ok("Офіційний Minecraft Launcher відкрито. Увійди в Microsoft-акаунт із ліцензією та запусти потрібну версію там.".to_string())
    }).await.map_err(|e| format!("Не вдалося запустити задачу: {e}"))?
}

#[derive(Serialize)]
struct JavaInfo { path: String, version: String, source: String }
pub(crate) fn data_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("VioletLauncher")
}
fn settings_file() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from("."))
        .join("VioletLauncher").join("settings.json")
}
#[tauri::command]
fn load_java_path() -> String {
    fs::read_to_string(settings_file()).ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.get("java_path").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_default()
}
#[tauri::command]
fn save_java_path(java_path: String) -> Result<(), String> {
    if !java_path.trim().is_empty() && !Path::new(&java_path).is_file() {
        return Err("За вказаним шляхом не знайдено файл. Вкажи повний шлях до java.exe.".into());
    }
    let path = settings_file();
    if let Some(parent) = path.parent() { fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
    fs::write(path, serde_json::to_vec_pretty(&json!({"java_path": java_path.trim()})).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())
}
#[tauri::command(async)]
fn detect_java() -> Vec<JavaInfo> {
    let mut found: Vec<JavaInfo> = Vec::new();
    let mut candidates: Vec<(PathBuf, String)> = Vec::new();
    if let Ok(output) = Command::new("where.exe").arg("java").output() {
        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                let p = PathBuf::from(line.trim());
                if p.exists() { candidates.push((p, "PATH".into())); }
            }
        }
    }
    if let Ok(program_files) = std::env::var("ProgramFiles") {
        for vendor in ["Java", "Eclipse Adoptium", "Microsoft", "Zulu"] {
            let p = PathBuf::from(&program_files).join(vendor);
            if let Ok(entries) = fs::read_dir(p) {
                for entry in entries.flatten() {
                    let exe = entry.path().join("bin").join("java.exe");
                    if exe.exists() { candidates.push((exe, "Встановлений runtime".into())); }
                }
            }
        }
    }
    if let Some(home) = dirs::home_dir() {
        for p in [home.join(".jdks"), home.join("scoop/apps/temurin"), home.join("scoop/apps/openjdk")] {
            if let Ok(entries) = fs::read_dir(p) {
                for entry in entries.flatten() {
                    let exe = entry.path().join("bin").join("java.exe");
                    if exe.exists() { candidates.push((exe, "Локальний runtime".into())); }
                }
            }
        }
    }
    for (path, source) in candidates {
        let canonical = path.canonicalize().unwrap_or(path.clone());
        if found.iter().any(|x| PathBuf::from(&x.path) == canonical) { continue; }
        let version = Command::new(&canonical).arg("-version").output().ok()
            .map(|o| format!("{}{}", String::from_utf8_lossy(&o.stderr), String::from_utf8_lossy(&o.stdout)))
            .and_then(|s| s.lines().next().map(|l| l.trim_matches('"').to_string()))
            .unwrap_or_else(|| "Версію не визначено".into());
        found.push(JavaInfo { path: canonical.to_string_lossy().to_string(), version, source });
    }
    found
}

#[derive(Serialize)]
struct ModPage { items: Vec<ModProject>, total: u64 }
#[derive(Serialize)]
struct ModProject {
    id: String, title: String, description: String, author: String,
    downloads: u64, icon_url: String, source: String, latest_file_id: Option<String>,
    latest_file_name: Option<String>, mod_id: Option<u64>,
}
#[tauri::command]
async fn search_modrinth(query: String, game_version: String, loader: String, project_type: String, offset: u32) -> Result<ModPage, String> {
    tauri::async_runtime::spawn_blocking(move || search_modrinth_blocking(query, game_version, loader, project_type, offset))
        .await.map_err(|e| format!("Фонова задача перервалася: {e}"))?
}
fn search_modrinth_blocking(query: String, game_version: String, loader: String, project_type: String, offset: u32) -> Result<ModPage, String> {
    let client = http()?;
    let mut url = reqwest::Url::parse("https://api.modrinth.com/v2/search").map_err(|e| e.to_string())?;
    url.query_pairs_mut().append_pair("query", &query).append_pair("limit", "24").append_pair("offset", &offset.to_string()).append_pair("index", "downloads");
    let mut facets: Vec<Vec<String>> = vec![vec![format!("project_type:{project_type}")]];
    if !game_version.trim().is_empty() { facets.push(vec![format!("versions:{}", game_version.trim())]); }
    if !loader.is_empty() && loader != "vanilla" { facets.push(vec![format!("categories:{loader}")]); }
    url.query_pairs_mut().append_pair("facets", &serde_json::to_string(&facets).unwrap_or_default());
    let data: Value = client.get(url).send().map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?
        .json().map_err(|e| e.to_string())?;
    let hits = data.get("hits").and_then(Value::as_array).ok_or("Modrinth повернув некоректну відповідь")?;
    let total = data.get("total_hits").and_then(Value::as_u64).unwrap_or(hits.len() as u64);
    let items = hits.iter().filter_map(|x| Some(ModProject {
        id: x.get("project_id")?.as_str()?.into(),
        title: x.get("title")?.as_str()?.into(),
        description: x.get("description").and_then(Value::as_str).unwrap_or("").into(),
        author: x.get("author").and_then(Value::as_str).unwrap_or("Unknown").into(),
        downloads: x.get("downloads").and_then(Value::as_u64).unwrap_or(0),
        icon_url: x.get("icon_url").and_then(Value::as_str).unwrap_or("").into(),
        source: "Modrinth".into(), latest_file_id: None, latest_file_name: None, mod_id: None,
    })).collect();
    Ok(ModPage { items, total })
}
#[derive(Serialize)]
struct ModFile { id: String, name: String, version: String, download_url: String, filename: String }
#[tauri::command]
async fn get_modrinth_versions(project_id: String, game_version: String, loader: String) -> Result<Vec<ModFile>, String> {
    tauri::async_runtime::spawn_blocking(move || get_modrinth_versions_blocking(project_id, game_version, loader))
        .await.map_err(|e| format!("Фонова задача перервалася: {e}"))?
}
fn get_modrinth_versions_blocking(project_id: String, game_version: String, loader: String) -> Result<Vec<ModFile>, String> {
    if !safe_component(&project_id) { return Err("Некоректний ID проєкту".into()); }
    let mut url = reqwest::Url::parse(&format!("https://api.modrinth.com/v2/project/{project_id}/version")).map_err(|e| e.to_string())?;
    {
        let mut q = url.query_pairs_mut();
        if !game_version.is_empty() { q.append_pair("game_versions", &json!([game_version]).to_string()); }
        if !loader.is_empty() && loader != "vanilla" { q.append_pair("loaders", &json!([loader]).to_string()); }
    }
    let data: Value = http()?.get(url).send().map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?.json().map_err(|e| e.to_string())?;
    let arr = data.as_array().ok_or("Некоректний список версій Modrinth")?;
    Ok(arr.iter().filter_map(|v| {
        let file = v.get("files")?.as_array()?.iter().find(|f| f.get("primary").and_then(Value::as_bool) == Some(true))
            .or_else(|| v.get("files")?.as_array()?.first())?;
        Some(ModFile {
            id: v.get("id")?.as_str()?.into(),
            name: v.get("name").and_then(Value::as_str).unwrap_or("Latest").into(),
            version: v.get("version_number").and_then(Value::as_str).unwrap_or("").into(),
            download_url: file.get("url")?.as_str()?.into(),
            filename: file.get("filename")?.as_str()?.into(),
        })
    }).collect())
}
fn install_modrinth_tree(project_id: &str, game_version: &str, loader: &str, dir: &Path, visited: &mut std::collections::HashSet<String>) -> Result<Vec<String>, String> {
    if !visited.insert(project_id.to_string()) { return Ok(Vec::new()); }
    if visited.len() > 80 { return Err("Забагато залежностей (ліміт 80)".into()); }
    let client = http()?;
    let mut url = reqwest::Url::parse(&format!("https://api.modrinth.com/v2/project/{project_id}/version")).map_err(|e| e.to_string())?;
    { let mut q = url.query_pairs_mut(); q.append_pair("game_versions", &json!([game_version]).to_string()); if loader != "" && loader != "vanilla" { q.append_pair("loaders", &json!([loader]).to_string()); } }
    let versions: Value = client.get(url).send().map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?.json().map_err(|e| e.to_string())?;
    let version = versions.as_array().and_then(|a| a.first()).ok_or_else(|| format!("Немає сумісної версії або залежностей для {project_id}"))?;
    let mut output = Vec::new();
    if let Some(deps) = version.get("dependencies").and_then(Value::as_array) {
        for dep in deps {
            if dep.get("dependency_type").and_then(Value::as_str) == Some("required") {
                if let Some(id) = dep.get("project_id").and_then(Value::as_str) {
                    output.extend(install_modrinth_tree(id, game_version, loader, dir, visited)?);
                }
            }
        }
    }
    if let Some(files) = version.get("files").and_then(Value::as_array) {
        if let Some(file) = files.iter().find(|f| f.get("primary").and_then(Value::as_bool)==Some(true)).or_else(|| files.first()) {
            let name = file.get("filename").and_then(Value::as_str).ok_or("Файл моду без імені")?;
            if name.contains("..") || name.contains('/') || name.contains('\\') { return Err("Некоректне ім’я файлу моду".into()); }
            let download = file.get("url").and_then(Value::as_str).ok_or("Немає URL файлу моду")?;
            if !download.starts_with("https://cdn.modrinth.com/") { return Err("Невідоме джерело файлу моду".into()); }
            let path = dir.join(name); download_to(download, &path)?;
            output.push(path.to_string_lossy().to_string());
        }
    }
    Ok(output)
}
#[tauri::command]
async fn install_modrinth_with_dependencies(project_id: String, game_version: String, loader: String, mods_dir: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let dir = PathBuf::from(mods_dir); fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        install_modrinth_tree(&project_id, &game_version, &loader, &dir, &mut std::collections::HashSet::new())
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn download_mod_file(url: String, filename: String, mods_dir: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || download_mod_file_blocking(url, filename, mods_dir))
        .await.map_err(|e| format!("Фонова задача перервалася: {e}"))?
}
fn download_mod_file_blocking(url: String, filename: String, mods_dir: String) -> Result<String, String> {
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') { return Err("Некоректне ім’я файлу".into()); }
    if !(url.starts_with("https://cdn.modrinth.com/") || url.starts_with("https://edge.forgecdn.net/") || url.starts_with("https://mediafilez.forgecdn.net/")) {
        return Err("Дозволено завантаження лише з відомих CDN Modrinth/CurseForge".into());
    }
    let dir = PathBuf::from(&mods_dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(&filename);
    download_to(&url, &path)?;
    Ok(path.to_string_lossy().to_string())
}

#[tauri::command]
async fn list_instance_mods(game_dir: String, instance_id: String) -> Result<Vec<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !safe_component(&instance_id) { return Err("Некоректний ID збірки".into()); }
        let dir = PathBuf::from(game_dir).join("instances").join(instance_id).join("mods");
        if !dir.exists() { return Ok(Vec::new()); }
        let mut files = Vec::new();
        for entry in fs::read_dir(dir).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            if entry.file_type().map_err(|e| e.to_string())?.is_file() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.to_ascii_lowercase().ends_with(".jar") { files.push(name); }
            }
        }
        files.sort(); Ok(files)
    }).await.map_err(|e| e.to_string())?
}
#[tauri::command]
async fn delete_instance_mod(game_dir: String, instance_id: String, filename: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !safe_component(&instance_id) || filename.contains("..") || filename.contains('/') || filename.contains('\\') || !filename.to_ascii_lowercase().ends_with(".jar") { return Err("Некоректний шлях до моду".into()); }
        let path = PathBuf::from(game_dir).join("instances").join(instance_id).join("mods").join(filename);
        fs::remove_file(path).map_err(|e| format!("Не вдалося видалити мод: {e}"))
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn search_curseforge(query: String, game_version: String, loader: String, api_key: String, offset: u32) -> Result<ModPage, String> {
    tauri::async_runtime::spawn_blocking(move || search_curseforge_blocking(query, game_version, loader, api_key, offset))
        .await.map_err(|e| format!("Фонова задача перервалася: {e}"))?
}
fn search_curseforge_blocking(query: String, game_version: String, loader: String, api_key: String, offset: u32) -> Result<ModPage, String> {
    if api_key.trim().is_empty() { return Err("Для CurseForge потрібен власний API key. Отримай його в порталі розробника CurseForge і введи в налаштуваннях лаунчера.".into()); }
    let mut url = reqwest::Url::parse("https://api.curseforge.com/v1/mods/search").map_err(|e| e.to_string())?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("gameId", "432").append_pair("classId", "6").append_pair("pageSize", "24").append_pair("index", &offset.to_string()).append_pair("sortField", "2").append_pair("sortOrder", "desc");
        if !query.trim().is_empty() { q.append_pair("searchFilter", query.trim()); }
        if !game_version.is_empty() { q.append_pair("gameVersion", &game_version); }
        let loader_id = match loader.as_str() { "forge" => Some("1"), "fabric" => Some("4"), "quilt" => Some("5"), "neoforge" => Some("6"), _ => None };
        if let Some(id) = loader_id { q.append_pair("modLoaderType", id); }
    }
    let data: Value = http()?.get(url).header("x-api-key", api_key.trim()).send().map_err(|e| e.to_string())?
        .error_for_status().map_err(|e| format!("CurseForge API: {e}"))?.json().map_err(|e| e.to_string())?;
    let hits = data.get("data").and_then(Value::as_array).ok_or("CurseForge повернув некоректну відповідь")?;
    let total = data.pointer("/pagination/totalCount").and_then(Value::as_u64).unwrap_or(hits.len() as u64);
    let items = hits.iter().filter_map(|x| {
        let id = x.get("id")?.as_u64()?;
        let latest = x.get("latestFiles")?.as_array()?.first();
        Some(ModProject {
            id: id.to_string(),
            title: x.get("name")?.as_str()?.into(),
            description: x.get("summary").and_then(Value::as_str).unwrap_or("").into(),
            author: x.pointer("/authors/0/name").and_then(Value::as_str).unwrap_or("Unknown").into(),
            downloads: x.get("downloadCount").and_then(Value::as_u64).unwrap_or(0),
            icon_url: x.pointer("/logo/url").and_then(Value::as_str).unwrap_or("").into(),
            source: "CurseForge".into(),
            latest_file_id: latest.and_then(|f| f.get("id")).and_then(Value::as_u64).map(|n| n.to_string()),
            latest_file_name: latest.and_then(|f| f.get("fileName")).and_then(Value::as_str).map(str::to_string),
            mod_id: Some(id),
        })
    }).collect();
    Ok(ModPage { items, total })
}
#[tauri::command]
async fn download_curseforge_mod(mod_id: String, file_id: String, api_key: String, mods_dir: String, game_version: String, loader: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || download_curseforge_mod_blocking(mod_id, file_id, api_key, mods_dir, game_version, loader))
        .await.map_err(|e| format!("Фонова задача перервалася: {e}"))?
}
fn download_curseforge_mod_blocking(mod_id: String, file_id: String, api_key: String, mods_dir: String, game_version: String, loader: String) -> Result<String, String> {
    if api_key.trim().is_empty() { return Err("Вкажи CurseForge API key".into()); }
    let mut files_url = reqwest::Url::parse(&format!("https://api.curseforge.com/v1/mods/{mod_id}/files")).map_err(|e| e.to_string())?;
    {
        let mut q = files_url.query_pairs_mut();
        q.append_pair("gameVersion", &game_version).append_pair("pageSize", "50");
        let loader_id = match loader.as_str() { "forge" => Some("1"), "fabric" => Some("4"), "quilt" => Some("5"), "neoforge" => Some("6"), _ => None };
        if let Some(id) = loader_id { q.append_pair("modLoaderType", id); }
    }
    let files: Value = http()?.get(files_url).header("x-api-key", api_key.trim()).send().map_err(|e| e.to_string())?
        .error_for_status().map_err(|e| format!("CurseForge API: {e}"))?.json().map_err(|e| e.to_string())?;
    let file_list = files.get("data").and_then(Value::as_array).ok_or("CurseForge не повернув список файлів")?;
    let chosen = file_list.iter().find(|f| f.get("id").and_then(Value::as_u64).map(|id| id.to_string()) == Some(file_id.clone()))
        .or_else(|| file_list.first()).ok_or("Не знайдено файл для вибраної версії гри. Спробуй інший loader або версію.")?;
    let chosen_id = chosen.get("id").and_then(Value::as_u64).ok_or("Некоректний ID файлу CurseForge")?;
    let filename = chosen.get("fileName").and_then(Value::as_str).unwrap_or("curseforge-mod.jar").to_string();
    if filename.contains("..") || filename.contains('/') || filename.contains('\\') { return Err("Некоректне ім’я файлу".into()); }
    let url = format!("https://api.curseforge.com/v1/mods/{mod_id}/files/{chosen_id}/download-url");
    let data: Value = http()?.get(url).header("x-api-key", api_key.trim()).send().map_err(|e| e.to_string())?
        .error_for_status().map_err(|e| format!("CurseForge API: {e}"))?.json().map_err(|e| e.to_string())?;
    let download = data.get("data").and_then(Value::as_str).ok_or("CurseForge не надав прямий URL. Можливо, файл недоступний для стороннього завантаження.")?;
    if !(download.starts_with("https://edge.forgecdn.net/") || download.starts_with("https://mediafilez.forgecdn.net/") || download.starts_with("https://www.curseforge.com/")) {
        return Err("CurseForge повернув невідомий домен завантаження; файл не завантажено з міркувань безпеки.".into());
    }
    let dir = PathBuf::from(mods_dir);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(filename);
    download_to(download, &path)?;
    Ok(path.to_string_lossy().to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            default_game_dir, list_minecraft_versions, install_minecraft_version, open_official_launcher, detect_java, load_java_path, save_java_path,
            search_modrinth, get_modrinth_versions, download_mod_file, install_modrinth_with_dependencies,
            search_curseforge, download_curseforge_mod,
            list_instance_mods, delete_instance_mod,
            launch::launch_offline, launch::offline_profile,
            profiles::list_profiles, profiles::save_profile, profiles::delete_profile,
            loaders::loader_versions, loaders::install_with_loader, loaders::installed_versions, loaders::loader_version_id,
            skins::list_skins, skins::add_skin, skins::delete_skin, skins::set_active_skin, skins::import_skin_by_username
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
