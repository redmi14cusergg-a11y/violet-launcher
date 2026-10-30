# Офлайн-авторизація

- `src-tauri/src/offline_auth.rs` — UUID v3 (MD5 від `OfflinePlayer:<нік>`, як у Java `nameUUIDFromBytes`), `OfflineSession`, аргументи `--username/--uuid/--accessToken/--userType legacy`.
- `src-tauri/src/launch.rs` — команда `launch_offline`: читає `versions/<v>/<v>.json`, збирає classpath, підставляє аргументи й запускає Java.
- UI: поле «Офлайн-нікнейм» на сторінці версій; кнопка «Запустити гру».

Обмеження: лише vanilla-версії (без inheritsFrom), бібліотеки/natives — за Windows-правилами проєкту; для дуже старих версій (≤1.12) natives треба розпаковувати окремо.
