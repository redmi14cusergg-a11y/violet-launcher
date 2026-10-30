//! Офлайн-запуск встановленої версії Minecraft (vanilla) без авторизації Microsoft/Mojang.

use crate::offline_auth::OfflineSession;
use crate::{allowed_for_windows, emit_progress, ensure_game_files, Progress, load_java_path, rel_library_path, safe_component};
use serde_json::Value;
use std::{fs, path::PathBuf, process::Command};

/// Обчислення `rules` з version.json (os/features). Правила з `features` (demo, custom resolution…) не активуємо.
fn rules_allow(rules: Option<&Value>) -> bool {
    let Some(rules) = rules.and_then(Value::as_array) else { return true; };
    let mut allowed = false;
    for rule in rules {
        let os_ok = match rule.get("os") {
            None => true,
            Some(os) => {
                let name_ok = os.get("name").and_then(Value::as_str).map(|n| n == "windows").unwrap_or(true);
                let arch_ok = os.get("arch").and_then(Value::as_str).map(|a| a != "x86" || cfg!(target_pointer_width = "32")).unwrap_or(true);
                name_ok && arch_ok
            }
        };
        let features_ok = rule.get("features").is_none();
        if os_ok && features_ok {
            allowed = rule.get("action").and_then(Value::as_str) == Some("allow");
        }
    }
    allowed
}

/// Розгортає масив `arguments.jvm` / `arguments.game` у плоский список рядків.
fn flatten_args(arr: Option<&Vec<Value>>) -> Vec<String> {
    let mut out = Vec::new();
    for item in arr.into_iter().flatten() {
        match item {
            Value::String(s) => out.push(s.clone()),
            Value::Object(o) => {
                if !rules_allow(o.get("rules")) { continue; }
                match o.get("value") {
                    Some(Value::String(s)) => out.push(s.clone()),
                    Some(Value::Array(vs)) => out.extend(vs.iter().filter_map(|v| v.as_str().map(str::to_string))),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    out
}

pub fn launch_offline_blocking(version: String, game_dir: String, username: String, java_path: String, memory_mb: u32, instance_id: String, extra_jvm_args: String, progress: Progress) -> Result<String, String> {
    if !safe_component(&version) { return Err("Некоректний ідентифікатор версії".into()); }
    let session = OfflineSession::new(&username).map_err(|e| match e {
        crate::offline_auth::UsernameError::Empty => "Введи нікнейм.".to_string(),
        crate::offline_auth::UsernameError::TooLong => "Нікнейм має бути не довшим за 16 символів.".to_string(),
        crate::offline_auth::UsernameError::InvalidChar(c) => format!("Недопустимий символ «{c}» у нікнеймі (дозволено A–Z, a–z, 0–9, _)."),
    })?;

    let base = PathBuf::from(&game_dir);
    let inst = if instance_id.trim().is_empty() { base.clone() } else {
        if !safe_component(&instance_id) { return Err("Некоректний ID збірки".into()); }
        base.join("instances").join(&instance_id)
    };
    fs::create_dir_all(&inst).map_err(|e| format!("Не вдалося створити папку збірки: {e}"))?;
    let inst_str = inst.to_string_lossy().to_string();
    let vdir = base.join("versions").join(&version);
    let json_path = vdir.join(format!("{version}.json"));
    if !json_path.is_file() {
        // Лише ванільні версії можна докачати автоматично; для завантажувачів — встановити на сторінці «Версії гри».
        if crate::loaders::detect_loader(&version) == "vanilla" {
            crate::install_minecraft_version_blocking(version.clone(), game_dir.clone(), progress)?;
        }
    }
    if !json_path.is_file() {
        return Err(format!("Версію {version} не встановлено. Встанови її на сторінці «Версії гри»."));
    }
    // Об'єднуємо version.json завантажувача (inheritsFrom) із ванільним
    let (vj, root_id) = crate::loaders::resolve_version(&base, &version)?;
    let jar_path = base.join("versions").join(&root_id).join(format!("{root_id}.jar"));
    if !jar_path.is_file() {
        crate::install_minecraft_version_blocking(root_id.clone(), game_dir.clone(), progress)?;
    }
    if !jar_path.is_file() {
        return Err(format!("Не знайдено клієнт Minecraft {root_id}. Спочатку натисни «Завантажити версію»."));
    }
    // Самовідновлення: докачуємо відсутні бібліотеки та ресурси (без мережі нічого не робить, якщо все на місці).
    ensure_game_files(&vj, &base, progress).map_err(|e| format!("Не вдалося докачати файли гри: {e}"))?;
    let main_class = vj.get("mainClass").and_then(Value::as_str).ok_or("У version.json немає mainClass")?.to_string();

    // classpath
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut cp: Vec<String> = Vec::new();
    for lib in vj.get("libraries").and_then(Value::as_array).into_iter().flatten() {
        if !allowed_for_windows(lib) { continue; }
        if let Some(rel) = rel_library_path(lib) {
            let p = base.join("libraries").join(rel);
            if p.is_file() { cp.push(p.to_string_lossy().to_string()); }
        }
    }
    cp.push(jar_path.to_string_lossy().to_string());
    let classpath = cp.join(sep);

    let natives_dir = vdir.join("natives");
    fs::create_dir_all(&natives_dir).map_err(|e| e.to_string())?;
    let assets_root = base.join("assets");
    let assets_index = vj.pointer("/assetIndex/id").and_then(Value::as_str).unwrap_or("legacy").to_string();

    let subst = |s: &str| -> String {
        [
            ("${auth_player_name}", session.username.as_str()),
            ("${auth_uuid}", session.uuid.as_str()),
            ("${auth_access_token}", session.access_token.as_str()),
            ("${auth_session}", session.access_token.as_str()),
            ("${user_type}", session.user_type.as_str()),
            ("${version_name}", version.as_str()),
            ("${version_type}", "release"),
            ("${game_directory}", inst_str.as_str()),
            ("${assets_root}", &assets_root.to_string_lossy()),
            ("${game_assets}", &assets_root.join("virtual").join("legacy").to_string_lossy()),
            ("${assets_index_name}", assets_index.as_str()),
            ("${natives_directory}", &natives_dir.to_string_lossy()),
            ("${launcher_name}", "VioletLauncher"),
            ("${launcher_version}", "0.3"),
            ("${classpath}", classpath.as_str()),
            ("${user_properties}", "{}"),
            ("${clientid}", "0"),
            ("${auth_xuid}", "0"),
            ("${library_directory}", &base.join("libraries").to_string_lossy()),
            ("${classpath_separator}", sep),
        ].iter().fold(s.to_string(), |acc, (k, v)| acc.replace(k, v))
    };

    let (mut jvm, mut game): (Vec<String>, Vec<String>) = if let Some(args) = vj.get("arguments") {
        (flatten_args(args.get("jvm").and_then(Value::as_array)), flatten_args(args.get("game").and_then(Value::as_array)))
    } else {
        // Старий формат (≤1.12): один рядок minecraftArguments
        let g = vj.get("minecraftArguments").and_then(Value::as_str).unwrap_or("")
            .split_whitespace().map(str::to_string).collect();
        (vec![format!("-Djava.library.path={}", natives_dir.to_string_lossy()), "-cp".into(), classpath.clone()], g)
    };
    jvm = jvm.iter().map(|a| subst(a)).collect();
    game = game.iter().map(|a| subst(a)).collect();

    // Якщо шаблон не містив потрібних прапорців — додаємо їх з офлайн-сесії.
    if !game.iter().any(|a| a == "--username") {
        game.extend(session.game_args());
    }

    let java = if !java_path.trim().is_empty() { java_path } else {
        let saved = load_java_path();
        if saved.trim().is_empty() { "java".to_string() } else { saved }
    };
    let mut cmd = Command::new(&java);
    if !jvm.iter().any(|a| a.starts_with("-Xmx")) { cmd.arg(format!("-Xmx{}M", memory_mb.max(512))); }
    // Користувацькі JVM-аргументи: один аргумент на рядок, щоб зберегти пробіли всередині значень.
    let custom: Vec<String> = extra_jvm_args.lines().map(str::trim).filter(|x| !x.is_empty()).map(str::to_string).collect();
    cmd.args(&custom).args(&jvm).arg(&main_class).args(&game).current_dir(&inst);
    // Скін (ресурс-пак зі стандартними скінами) — некритично, помилка не зупиняє запуск.
    if let Err(e) = crate::skins::apply_skin(&inst, &version, crate::skins::active_skin().as_deref()) { eprintln!("skin: {e}"); }
    cmd.spawn().map_err(|e| format!("Не вдалося запустити Java ({java}): {e}"))?;
    Ok(format!("Minecraft {version} запущено офлайн як «{}» (UUID {}).", session.username, session.uuid))
}

#[tauri::command]
pub async fn launch_offline(app: tauri::AppHandle, version: String, game_dir: String, username: String, java_path: String, memory_mb: u32, instance_id: String, extra_jvm_args: String) -> Result<String, String> {
    tauri::async_runtime::spawn_blocking(move || launch_offline_blocking(version, game_dir, username, java_path, memory_mb, instance_id, extra_jvm_args, &|d, t, lb| emit_progress(&app, d, t, lb)))
        .await.map_err(|e| format!("Задача запуску перервалася: {e}"))?
}

/// Повертає дані офлайн-профілю (для відображення в UI).
#[tauri::command]
pub fn offline_profile(username: String) -> Result<OfflineSession, String> {
    OfflineSession::new(&username).map_err(|e| format!("{e:?}"))
}
