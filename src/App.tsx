import { useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import "./App.css";

type Page = "home" | "versions" | "mods" | "skins" | "settings";
const MOD_PAGE_SIZE = 24;
type MinecraftVersion = { id: string; kind: string; release_time: string };
type Profile = { id: string; name: string; version: string; username: string; memory_mb: number; loader: string; jvm_args: string };
type InstalledVersion = { id: string; mc_version: string; loader: string };
const LOADERS: [string, string][] = [["vanilla","Vanilla"],["fabric","Fabric"],["quilt","Quilt"],["forge","Forge"],["neoforge","NeoForge"]];
const loaderTitle = (l: string) => LOADERS.find(x => x[0] === l)?.[1] ?? "Vanilla";
type SkinInfo = { name: string; data_url: string; active: boolean };
type ModPageData = { items: ModProject[]; total: number };
type JavaInfo = { path: string; version: string; source: string };
type ModProject = {
  id: string; title: string; description: string; author: string; downloads: number;
  icon_url: string; source: string; latest_file_id: string | null;
  latest_file_name: string | null; mod_id: number | null;
};

const navItems: { id: Page; label: string; icon: string }[] = [
  { id: "home", label: "Головна", icon: "⌂" },
  { id: "versions", label: "Версії гри", icon: "◈" },
  { id: "mods", label: "Моди", icon: "▦" },
  { id: "skins", label: "Скіни", icon: "☺" },
  { id: "settings", label: "Java та налаштування", icon: "⚙" },
];
const prettySize = (n: number) => n >= 1_000_000 ? `${(n / 1_000_000).toFixed(1)} млн` : n.toLocaleString("uk-UA");


function pageNumbers(cur: number, total: number): (number | "gap")[] {
  const set = new Set([0, total - 1, cur - 1, cur, cur + 1]);
  const nums = [...set].filter(n => n >= 0 && n < total).sort((a, b) => a - b);
  const out: (number | "gap")[] = [];
  nums.forEach((n, i) => { if (i && n - nums[i - 1] > 1) out.push("gap"); out.push(n); });
  return out;
}

/** Передній вигляд персонажа зі скіна (з другим шаром) на canvas. */
function SkinFigure({ src }: { src: string }) {
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const img = new Image();
    img.onload = () => {
      const c = ref.current; const g = c?.getContext("2d");
      if (!c || !g) return;
      g.imageSmoothingEnabled = false; g.clearRect(0, 0, c.width, c.height);
      const k = 5;
      const part = (sx: number, sy: number, w: number, h: number, dx: number, dy: number) => g.drawImage(img, sx, sy, w, h, dx * k, dy * k, w * k, h * k);
      // база
      part(8, 8, 8, 8, 4, 0); part(20, 20, 8, 12, 4, 8); part(44, 20, 4, 12, 0, 8); part(36, 52, 4, 12, 12, 8); part(4, 20, 4, 12, 4, 20); part(20, 52, 4, 12, 8, 20);
      // другий шар
      part(40, 8, 8, 8, 4, 0); part(20, 36, 8, 12, 4, 8); part(44, 36, 4, 12, 0, 8); part(52, 52, 4, 12, 12, 8); part(4, 36, 4, 12, 4, 20); part(4, 52, 4, 12, 8, 20);
    };
    img.src = src;
  }, [src]);
  return <canvas ref={ref} width={80} height={160} className="skin-figure" aria-hidden="true" />;
}
function App() {
  const [page, setPage] = useState<Page>("home");
  const [versions, setVersions] = useState<MinecraftVersion[]>([]);
  const [version, setVersion] = useState("1.21.4");
  const [versionFilter, setVersionFilter] = useState("release");
  const [gameDir, setGameDir] = useState("");
  const [javaList, setJavaList] = useState<JavaInfo[]>([]);
  const [javaPath, setJavaPath] = useState("");
  const [playerName, setPlayerName] = useState("Player");
  const [notice, setNotice] = useState("");
  const [busy, setBusy] = useState(false);
  const [launching, setLaunching] = useState(false);
  const [installLoader, setInstallLoader] = useState("vanilla");
  const [loaderVersions, setLoaderVersions] = useState<string[]>([]);
  const [loaderVersion, setLoaderVersion] = useState("");
  const [loaderNotice, setLoaderNotice] = useState("");
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [editing, setEditing] = useState<Profile | null>(null);
  const [profileMods, setProfileMods] = useState<string[]>([]);
  const [modsBusy, setModsBusy] = useState(false);
  const [playingId, setPlayingId] = useState("");
  const [confirmDel, setConfirmDel] = useState("");
  const [skins, setSkins] = useState<SkinInfo[]>([]);
  const [skinName, setSkinName] = useState("");
  const [skinUser, setSkinUser] = useState("");
  const [skinNotice, setSkinNotice] = useState("");
  const [skinBusy, setSkinBusy] = useState(false);
  const [modPage, setModPage] = useState(0);
  const [modTotal, setModTotal] = useState(0);
  const [modTarget, setModTarget] = useState("");
  const [searchTick, setSearchTick] = useState(0);
  const reqId = useRef(0);
  const [progress, setProgress] = useState<{done:number; total:number; label:string} | null>(null);
  const [modSource, setModSource] = useState<"Modrinth" | "CurseForge">("Modrinth");
  const [modQuery, setModQuery] = useState("");
  const [modVersion, setModVersion] = useState("1.21.4");
  const [loader, setLoader] = useState("fabric");
  const [projectType, setProjectType] = useState("mod");
  const [mods, setMods] = useState<ModProject[]>([]);
  const [curseKey, setCurseKey] = useState("");
  const [modNotice, setModNotice] = useState("");
  const [selectedMods, setSelectedMods] = useState<string[]>([]);
  const [searching, setSearching] = useState(false);

  useEffect(() => {
    const unlisten = listen<{done:number; total:number; label:string}>("download-progress", e => setProgress(e.payload));
    return () => { unlisten.then(f => f()).catch(() => {}); };
  }, []);

  useEffect(() => {
    invoke<string>("default_game_dir").then(setGameDir).catch(() => setGameDir("C:\\Users\\Administrator\\AppData\\Roaming\\.minecraft"));
    invoke<MinecraftVersion[]>("list_minecraft_versions")
      .then((list) => { setVersions(list); const latest = list.find(v => v.kind === "release"); if (latest) { setVersion(latest.id); setModVersion(latest.id); } })
      .catch((e) => setNotice(`Не вдалося отримати список версій: ${String(e)}`));
    invoke<JavaInfo[]>("detect_java").then(setJavaList).catch(() => {});
    invoke<string>("load_java_path").then(setJavaPath).catch(() => {});
    invoke<Profile[]>("list_profiles").then(setProfiles).catch(() => {});
  }, []);


  useEffect(() => {
    setLoaderNotice(""); setLoaderVersions([]); setLoaderVersion("");
    if (installLoader === "vanilla") return;
    let alive = true;
    invoke<string[]>("loader_versions", { loader: installLoader, mcVersion: version })
      .then(list => { if (alive) { setLoaderVersions(list); setLoaderVersion(list[0] ?? ""); } })
      .catch(e => { if (alive) setLoaderNotice(String(e)); });
    return () => { alive = false; };
  }, [installLoader, version]);

  const needsLoaderVersion = installLoader !== "vanilla";
  const versionIdToRun = async () => needsLoaderVersion && loaderVersion
    ? await invoke<string>("loader_version_id", { mcVersion: version, loader: installLoader, loaderVersion })
    : version;

  async function installVersion() {
    if (!gameDir.trim()) { setNotice("Вкажи папку гри."); return; }
    if (needsLoaderVersion && !loaderVersion) { setNotice("Обери версію завантажувача."); return; }
    setBusy(true); setNotice(needsLoaderVersion ? `Встановлення ${loaderTitle(installLoader)} для Minecraft ${version}… Це може зайняти час.` : `Завантаження Minecraft ${version}… Це може зайняти час.`);
    try {
      if (needsLoaderVersion) {
        const r = await invoke<{version_id:string; message:string}>("install_with_loader", { mcVersion: version, loader: installLoader, loaderVersion, gameDir, javaPath });
        setNotice(`✓ ${r.message}`);
      } else {
        const result = await invoke<{version:string; directory:string; files_downloaded:number; message:string}>("install_minecraft_version", { version, gameDir });
        setNotice(`✓ ${result.message} Нових файлів: ${result.files_downloaded}. Папка: ${result.directory}`);
      }
    } catch (e) { setNotice(`Помилка встановлення: ${String(e)}`); }
    finally { setBusy(false); setProgress(null); }
  }

  async function launchGame() {
    if (!gameDir.trim()) { setNotice("Спочатку вкажи папку гри."); return; }
    setLaunching(true);
    setNotice(`Запускаю Minecraft ${version} офлайн як «${playerName}»…`);
    try {
      const runId = await versionIdToRun();
      const message = await invoke<string>("launch_offline", { version: runId, gameDir, username: playerName, javaPath, memoryMb: 2048, instanceId: "" });
      setNotice(`✓ ${message}`);
    } catch (e) {
      setNotice(`Не вдалося запустити гру: ${String(e)}`);
    } finally {
      setLaunching(false);
      setProgress(null);
    }
  }

  const searchMods = () => { setModPage(0); setSearchTick(t => t + 1); };

  // Моди показуються одразу (найпопулярніші), а також при зміні фільтрів і сторінки.
  useEffect(() => {
    if (page !== "mods") return;
    if (modSource === "CurseForge" && !curseKey.trim()) {
      setMods([]); setModTotal(0); setModNotice("Для CurseForge введи свій API key — після цього тут з’являться моди."); return;
    }
    const id = ++reqId.current;
    setSearching(true); setModNotice("");
    const offset = modPage * MOD_PAGE_SIZE;
    const request = modSource === "Modrinth"
      ? invoke<ModPageData>("search_modrinth", { query: modQuery, gameVersion: modVersion, loader, projectType, offset })
      : invoke<ModPageData>("search_curseforge", { query: modQuery, gameVersion: modVersion, loader, apiKey: curseKey, offset });
    request.then(r => {
      if (id !== reqId.current) return;
      setMods(r.items); setModTotal(r.total);
      if (!r.items.length) setModNotice("Нічого не знайдено. Спробуй інший запит або версію гри.");
    }).catch(e => { if (id === reqId.current) { setMods([]); setModTotal(0); setModNotice(String(e)); } })
      .finally(() => { if (id === reqId.current) setSearching(false); });
  }, [page, modSource, modVersion, loader, projectType, modPage, searchTick]);

  useEffect(() => {
    if (!modTarget) return;
    const p = profiles.find(x => x.id === modTarget);
    if (p) { setLoader(p.loader || "fabric"); setModVersion(p.version); setModPage(0); }
  }, [modTarget]);

  const refreshSkins = () => invoke<SkinInfo[]>("list_skins").then(setSkins).catch(e => setSkinNotice(String(e)));
  useEffect(() => { if (page === "skins") refreshSkins(); }, [page]);

  async function uploadSkin(file: File | undefined) {
    if (!file) return;
    const name = (skinName.trim() || file.name.replace(/\.png$/i, "")).slice(0, 32);
    try {
      const bytes = Array.from(new Uint8Array(await file.arrayBuffer()));
      await invoke("add_skin", { name, bytes });
      setSkinNotice(`✓ Скін «${name}» додано.`); setSkinName(""); refreshSkins();
    } catch (e) { setSkinNotice(String(e)); }
  }
  async function importSkin() {
    if (!skinUser.trim()) { setSkinNotice("Введи нікнейм гравця."); return; }
    setSkinBusy(true); setSkinNotice("Завантажую скін із Mojang…");
    try { const n = await invoke<string>("import_skin_by_username", { username: skinUser.trim() }); setSkinNotice(`✓ Скін «${n}» імпортовано.`); setSkinUser(""); refreshSkins(); }
    catch (e) { setSkinNotice(String(e)); }
    finally { setSkinBusy(false); }
  }
  async function activateSkin(name: string) {
    try { await invoke("set_active_skin", { name }); setSkinNotice(name ? `✓ Скін «${name}» буде застосовано при наступному запуску гри.` : "Скін вимкнено — буде стандартний."); refreshSkins(); }
    catch (e) { setSkinNotice(String(e)); }
  }
  async function removeSkin(name: string) { try { await invoke("delete_skin", { name }); refreshSkins(); } catch (e) { setSkinNotice(String(e)); } }

  const refreshProfiles = () => invoke<Profile[]>("list_profiles").then(setProfiles).catch(() => {});
  async function saveProfile() {
    if (!editing) return;
    if (!gameDir.trim()) { setNotice("Спочатку вкажи папку Minecraft у розділі «Версії гри»."); return; }
    setBusy(true);
    try {
      // Профіль із модлоадером має отримати реальну встановлену версію до збереження.
      if (editing.loader && editing.loader !== "vanilla") {
        setNotice(`Отримую доступні версії ${loaderTitle(editing.loader)} для Minecraft ${editing.version}…`);
        const available = await invoke<string[]>("loader_versions", { loader: editing.loader, mcVersion: editing.version });
        if (!available.length) throw new Error(`${loaderTitle(editing.loader)} не має сумісних версій для Minecraft ${editing.version}.`);
        const selectedLoaderVersion = available[0];
        setNotice(`Встановлюю ${loaderTitle(editing.loader)} ${selectedLoaderVersion}…`);
        const installedLoader = await invoke<{version_id:string; message:string}>("install_with_loader", {
          mcVersion: editing.version, loader: editing.loader, loaderVersion: selectedLoaderVersion, gameDir, javaPath
        });
        setNotice(`✓ ${installedLoader.message}`);
      }
      await invoke("save_profile", { profile: editing });
      setEditing(null);
      refreshProfiles();
    } catch (e) { setNotice(`Не вдалося зберегти збірку: ${String(e)}`); }
    finally { setBusy(false); setProgress(null); }
  }
  async function removeProfile(id: string) {
    if (confirmDel !== id) { setConfirmDel(id); return; }
    setConfirmDel("");
    try { await invoke("delete_profile", { id }); refreshProfiles(); } catch (e) { setNotice(String(e)); }
  }
  async function launchProfile(p: Profile) {
    if (!gameDir.trim()) { setNotice("Спочатку вкажи папку гри."); return; }
    setLaunching(true); setPlayingId(p.id); setNotice(`Запускаю збірку «${p.name}» (Minecraft ${p.version})…`);
    try {
      let runVersion = p.version;
      if (p.loader && p.loader !== "vanilla") {
        const available = await invoke<string[]>("loader_versions", { loader: p.loader, mcVersion: p.version });
        if (!available.length) throw new Error(`Немає сумісної версії ${loaderTitle(p.loader)} для Minecraft ${p.version}.`);
        runVersion = await invoke<string>("loader_version_id", { mcVersion: p.version, loader: p.loader, loaderVersion: available[0] });
        const present = await invoke<InstalledVersion[]>("installed_versions", { gameDir });
        if (!present.some(v => v.id === runVersion)) {
          setNotice(`Встановлюю ${loaderTitle(p.loader)} перед запуском…`);
          const result = await invoke<{version_id:string; message:string}>("install_with_loader", { mcVersion: p.version, loader: p.loader, loaderVersion: available[0], gameDir, javaPath });
          runVersion = result.version_id;
        }
      }
      const msg = await invoke<string>("launch_offline", { version: runVersion, gameDir, username: p.username, javaPath, memoryMb: p.memory_mb, instanceId: p.id, extraJvmArgs: p.jvm_args || "" });
      setNotice(`✓ ${msg}`);
    } catch (e) { setNotice(`Не вдалося запустити: ${String(e)}`); }
    finally { setLaunching(false); setPlayingId(""); setProgress(null); }
  }
  const newProfile = (): Profile => ({ id: "", name: "", version, username: playerName || "Player", memory_mb: 2048, loader: "fabric", jvm_args: "" });
  async function openProfileEditor(p: Profile) {
    setEditing({ ...p, jvm_args: p.jvm_args ?? "" }); setProfileMods([]);
    if (p.id) { setModsBusy(true); try { setProfileMods(await invoke<string[]>("list_instance_mods", { gameDir, instanceId: p.id })); } catch (e) { setNotice(String(e)); } finally { setModsBusy(false); } }
  }
  async function deleteProfileMod(filename: string) {
    if (!editing?.id) return;
    try { await invoke("delete_instance_mod", { gameDir, instanceId: editing.id, filename }); setProfileMods(await invoke<string[]>("list_instance_mods", { gameDir, instanceId: editing.id })); }
    catch (e) { setNotice(String(e)); }
  }

  async function installMod(mod: ModProject) {
    const root = gameDir.replace(/[\\/]$/, "") + (modTarget ? `\\instances\\${modTarget}` : "");
    const sub = modSource === "CurseForge" ? "mods" : projectType === "resourcepack" ? "resourcepacks" : projectType === "shader" ? "shaderpacks" : projectType === "modpack" ? "modpacks" : "mods";
    const modsDir = `${root}\\${sub}`;
    setModNotice(`Завантажую ${mod.title}…`);
    try {
      let path = "";
      if (mod.source === "Modrinth") {
        const installedFiles = await invoke<string[]>("install_modrinth_with_dependencies", { projectId: mod.id, gameVersion: modVersion, loader, modsDir });
        if (!installedFiles.length) throw new Error("Не знайдено сумісного файлу моду.");
        path = installedFiles.join(", ");
      } else {
        if (!mod.latest_file_id) throw new Error("CurseForge не повернув ID файлу для цього проєкту.");
        path = await invoke<string>("download_curseforge_mod", { modId: mod.id, fileId: mod.latest_file_id, apiKey: curseKey, modsDir, gameVersion: modVersion, loader });
      }
      setModNotice(`✓ Завантажено: ${path}`);
    } catch (e) { setModNotice(`Не вдалося завантажити: ${String(e)}`); }
  }

  const shownVersions = versions.filter(v => versionFilter === "all" || v.kind === versionFilter);

  return (
    <main className="app-shell">
      <aside className="sidebar">
        <div className="brand"><div className="brand-mark"><span>V</span></div><div className="brand-copy"><strong>VIOLET</strong><small>MINECRAFT LAUNCHER</small></div></div>
        <div className="sidebar-label">МЕНЮ</div>
        <nav className="navigation" aria-label="Основна навігація">
          {navItems.map(item => <button key={item.id} className={`nav-item ${page === item.id ? "active" : ""}`} onClick={() => { setPage(item.id); setNotice(""); }}>
            <span className="nav-icon">{item.icon}</span><span>{item.label}</span>{page === item.id && <span className="active-indicator" />}
          </button>)}
        </nav>
        <div className="sidebar-bottom"><div className="status-dot" /><div><strong>Локальний профіль</strong><small>Режим розробки</small></div></div>
        <div className="sidebar-version">VIOLET LAUNCHER <span>0.2.0</span></div>
      </aside>

      <section className="main-panel">
        <header className="topbar"><div className="breadcrumbs"><span>Violet</span><b>/</b>{navItems.find(item => item.id === page)?.label}</div>
          <div className="topbar-right"><span className="online-pill"><i /> {busy || searching || launching ? "Виконується завдання" : "Лаунчер готовий"}</span><div className="avatar">{playerName.slice(0,1).toUpperCase() || "P"}</div></div>
        </header>

        {(busy || launching) && <div className="progress-card" role="progressbar" aria-valuemin={0} aria-valuemax={progress?.total ?? 0} aria-valuenow={progress?.done ?? 0}>
          <div className="progress-head"><strong>{launching ? "Підготовка до запуску" : "Завантаження гри"}</strong><span>{progress && progress.total > 0 ? `${progress.done} / ${progress.total} · ${Math.round(progress.done / progress.total * 100)}%` : "Підключення…"}</span></div>
          <div className={`progress-track ${progress && progress.total > 0 ? "" : "indeterminate"}`}><div className="progress-fill" style={{ width: progress && progress.total > 0 ? `${progress.done / progress.total * 100}%` : undefined }} /></div>
          <small>{progress?.label ?? "Отримую список файлів…"}</small>
        </div>}

        {page === "home" && <div className="page-content home-page">
          <div className="hero"><div className="hero-glow" /><div className="hero-copy"><div className="eyebrow"><span className="eyebrow-line" /> ТВОЯ ГРА. ТВОЇ ПРАВИЛА.</div>
            <h1>Час створювати<br/><span>свою пригоду.</span></h1><p>Керуй версіями Minecraft, налаштовуй Java<br className="desktop-break"/> та встановлюй моди з Modrinth і CurseForge.</p>
            <div className="hero-actions"><button className="play-button" onClick={() => setPage("versions")}><span className="play-triangle">↓</span> Завантажити версію <span className="button-arrow">↗</span></button><button className="secondary-button" onClick={() => setPage("mods")}>Знайти моди <span>→</span></button></div>
          </div><div className="hero-art" aria-hidden="true"><div className="orbit orbit-one"/><div className="orbit orbit-two"/><div className="cube cube-back"/><div className="cube cube-front"><div className="cube-face"/></div><div className="floating-chip chip-one">✦</div><div className="floating-chip chip-two">◈</div><div className="art-caption"><span>01</span><div><strong>Нова пригода</strong><small>Починається з тебе</small></div></div></div><div className="hero-index">01 <span>/</span> 04</div></div>
          <div className="section-heading"><div><span className="section-kicker">МОЇ ЗБІРКИ</span><h2>Швидкий запуск</h2></div><button className="secondary-button" onClick={() => { setProfileMods([]); setEditing(newProfile()); }}>＋ Нова збірка</button></div>
          {editing && <div className="modal-backdrop" onMouseDown={e => { if (e.target === e.currentTarget) setEditing(null); }}><section className="build-form build-modal" role="dialog" aria-modal="true" aria-label="Редагування збірки"><div className="modal-heading"><div><span className="section-kicker">НАЛАШТУВАННЯ ПРОФІЛЮ</span><h2>{editing.id ? "Редагувати збірку" : "Нова збірка"}</h2></div><button className="secondary-button" onClick={() => setEditing(null)}>✕</button></div>
            <label>Назва<input value={editing.name} maxLength={40} onChange={e => setEditing({ ...editing, name: e.target.value })} placeholder="Моя виживалка"/></label>
            <label>Версія Minecraft<select value={editing.version} onChange={e => setEditing({ ...editing, version: e.target.value })}>{[...new Set([editing.version, ...versions.filter(v => v.kind === "release").map(v => v.id)])].filter(v => !/^(fabric|quilt|forge|neoforge)[-:]?/i.test(v)).slice(0, 160).map(v => <option key={v} value={v}>{v}</option>)}</select></label>
            <label>Модлоадер<select value={editing.loader || "fabric"} onChange={e => setEditing({ ...editing, loader: e.target.value })}>{LOADERS.map(([id, title]) => <option key={id} value={id}>{title}</option>)}</select></label>
            <label>Нікнейм<input value={editing.username} maxLength={16} onChange={e => setEditing({ ...editing, username: e.target.value })}/></label>
            <label className="args-field">JVM аргументи (кожен з нового рядка)<textarea value={editing.jvm_args || ""} onChange={e => setEditing({ ...editing, jvm_args: e.target.value })} placeholder={"-XX:+UseG1GC\n-Dfile.encoding=UTF-8"} rows={3}/></label>
            <div className="installed-mods"><div className="modal-heading"><strong>Встановлені моди ({profileMods.length})</strong><small>{modsBusy ? "Завантажую список…" : "Папка цієї збірки"}</small></div>{profileMods.length ? profileMods.map(file => <div className="installed-mod-row" key={file}><span>▦ {file}</span><button className="secondary-button danger" onClick={() => deleteProfileMod(file)}>Видалити</button></div>) : <small className="muted">{modsBusy ? "" : editing.id ? "У цій збірці поки немає модів." : "Збережи збірку, щоб керувати її модами."}</small>}</div>
            <label>Пам’ять<select value={editing.memory_mb} onChange={e => setEditing({ ...editing, memory_mb: Number(e.target.value) })}>{[1024, 2048, 3072, 4096, 6144, 8192].map(m => <option key={m} value={m}>{m / 1024} ГБ</option>)}</select></label>
            <div className="build-form-actions"><button className="play-button" disabled={busy} onClick={saveProfile}>{busy ? "Зберігаю…" : "Зберегти"}</button><button className="secondary-button" onClick={() => setEditing(null)}>Скасувати</button></div>
          </section></div>}
          {notice && <div className="notice wide-notice" role="status">{notice}</div>}
          <div className="build-grid">{profiles.map(p => <article className="build-card" key={p.id}>
            <div className="build-top"><div className="quick-icon violet-icon">◈</div><div className="build-title"><strong>{p.name}</strong><small>{loaderTitle(p.loader || "vanilla")} · {p.version} · {p.username} · {p.memory_mb / 1024} ГБ</small></div></div>
            <div className="build-actions">
              <button className="play-button" disabled={launching} onClick={() => launchProfile(p)}>{playingId === p.id ? "Запускаю…" : "▶  Грати"}</button>
              <button className="secondary-button" onClick={() => openProfileEditor(p)} aria-label="Редагувати">✎</button>
              <button className={`secondary-button ${confirmDel === p.id ? "danger" : ""}`} onClick={() => removeProfile(p.id)} aria-label="Видалити">{confirmDel === p.id ? "Точно?" : "🗑"}</button>
            </div></article>)}
            {!profiles.length && !editing && <button className="build-empty" onClick={() => { setProfileMods([]); setEditing(newProfile()); }}><span>＋</span><strong>Створи першу збірку</strong><small>Окрема папка зі своїми модами, світами та налаштуваннями</small></button>}
          </div>
          <div className="section-heading"><div><span className="section-kicker">ШВИДКИЙ ДОСТУП</span><h2>Твій простір</h2></div><span className="subtle-label">УПРАВЛЯЙ ГРОЮ В ОДНОМУ МІСЦІ</span></div>
          <div className="quick-grid"><button className="quick-card" onClick={() => setPage("versions")}><div className="quick-icon violet-icon">◈</div><div className="quick-text"><strong>Версії гри</strong><small>Завантаження з Mojang</small></div><span className="card-arrow">↗</span></button>
          <button className="quick-card" onClick={() => setPage("mods")}><div className="quick-icon blue-icon">▦</div><div className="quick-text"><strong>Modrinth / CurseForge</strong><small>Пошук і встановлення модів</small></div><span className="card-arrow">↗</span></button>
          <button className="quick-card" onClick={() => setPage("settings")}><div className="quick-icon pink-icon">⚙</div><div className="quick-text"><strong>Java Runtime</strong><small>Вибір java.exe</small></div><span className="card-arrow">↗</span></button></div>
          <footer className="footer-note"><span>◆</span> Створено для твоїх наступних пригод <span className="footer-right">VIOLET / BUILT WITH TAURI</span></footer>
        </div>}

        {page === "versions" && <div className="page-content subpage">
          <div className="page-intro"><div className="eyebrow"><span className="eyebrow-line"/> ОФІЦІЙНИЙ МАНІФЕСТ MOJANG</div><h1>Версії <span>Minecraft</span></h1><p>Завантаження клієнта, бібліотек і ресурсів у вибрану папку гри.</p></div>
          <div className="settings-card"><div className="settings-card-icon">◈</div><div className="settings-card-copy"><h3>Папка Minecraft</h3><p>Вибери папку, в яку зберігати файли гри. За замовчуванням — .minecraft у AppData.</p><input value={gameDir} onChange={e => setGameDir(e.target.value)} placeholder="C:\\Users\\...\\.minecraft"/></div></div>
          <div className="settings-card"><div className="settings-card-icon">☺</div><div className="settings-card-copy"><h3>Офлайн-нікнейм</h3><p>Вхід без перевірки ліцензії: UUID генерується з нікнейма. Тільки A–Z, a–z, 0–9 та _, до 16 символів. Працює в одиночній грі та на серверах з online-mode=false.</p><input value={playerName} maxLength={16} onChange={e => setPlayerName(e.target.value)} placeholder="Steve"/></div></div>
          <div className="version-toolbar"><div className="segmented">{[["release","Релізи"],["snapshot","Снапшоти"],["all","Усі"]].map(([v,l])=><button key={v} className={versionFilter===v?"selected":""} onClick={()=>setVersionFilter(v)}>{l}</button>)}</div><span className="results-count">{shownVersions.length} версій у списку</span></div>
          <div className="version-list">{shownVersions.slice(0, 80).map(v=><button key={v.id} className={`version-row ${version===v.id?"selected":""}`} onClick={()=>setVersion(v.id)}><span className="version-gem">◈</span><span className="version-meta"><strong>{v.id}</strong><small>{v.kind === "release" ? "Стабільний реліз" : v.kind === "snapshot" ? "Snapshot / тестова збірка" : v.kind}</small></span><span className="version-date">{v.release_time.slice(0,10)}</span><span className="version-radio">{version===v.id?"✓":""}</span></button>)}</div>
          <div className="settings-card loader-card"><div className="settings-card-icon">⚒</div><div className="settings-card-copy"><h3>Завантажувач модів</h3><p>Fabric і Quilt ставляться миттєво. Forge і NeoForge запускають офіційний інсталятор (потрібна Java 17+) — це може тривати кілька хвилин.</p>
            <div className="segmented loader-seg">{LOADERS.map(([id, label]) => <button key={id} className={installLoader === id ? "selected" : ""} onClick={() => setInstallLoader(id)}>{label}</button>)}</div>
            {needsLoaderVersion && <div className="loader-version-row"><label>Версія {loaderTitle(installLoader)}<select value={loaderVersion} onChange={e => setLoaderVersion(e.target.value)} disabled={!loaderVersions.length}>{loaderVersions.map((v, i) => <option key={v} value={v}>{v}{installLoader === "forge" ? (i === 0 && loaderVersions.length > 1 ? " (рекомендована)" : " (остання)") : i === 0 ? " (остання)" : ""}</option>)}</select></label>
              {!loaderVersions.length && !loaderNotice && <small>Завантажую список…</small>}</div>}
            {loaderNotice && <small className="loader-error">{loaderNotice}</small>}</div></div>
          <div className="install-bar"><div><small>ВИБРАНА ВЕРСІЯ</small><strong>{needsLoaderVersion ? `${loaderTitle(installLoader)}${loaderVersion ? " " + loaderVersion : ""} · ` : ""}Minecraft {version}</strong></div><div className="install-actions"><button className="secondary-button" disabled={launching} onClick={launchGame}>{launching?"Запускаю…":"▶  Запустити гру"}</button><button className="play-button" disabled={busy || (needsLoaderVersion && !loaderVersion)} onClick={installVersion}>{busy?"Встановлюю…":needsLoaderVersion?"↓  Встановити з "+loaderTitle(installLoader):"↓  Завантажити версію"}</button></div></div>
          {notice && <div className="notice wide-notice" role="status">{notice}</div>}
          <div className="info-banner"><span>ⓘ</span><p><strong>Зверни увагу</strong><br/>Цей етап завантажує офіційний клієнт, JSON, бібліотеки та assets. Самостійний запуск гри потребує окремого етапу авторизації Microsoft і формування аргументів запуску.</p></div>
        </div>}

        {page === "mods" && <div className="page-content subpage mods-page">
          <div className="page-intro"><div className="eyebrow"><span className="eyebrow-line"/> КАТАЛОГИ МОДІВ</div><h1>Моди та <span>доповнення</span></h1><p>Шукай і завантажуй файли безпосередньо в папку mods.</p></div>
          <div className="provider-switch"><button className={modSource==="Modrinth"?"selected":""} onClick={()=>{setModSource("Modrinth");setMods([]);setModNotice("");setModPage(0);}}>◈ Modrinth <small>Відкритий API</small></button><button className={modSource==="CurseForge"?"selected":""} onClick={()=>{setModSource("CurseForge");setMods([]);setModNotice("");setModPage(0);}}>✦ CurseForge <small>Потрібен API key</small></button></div>
          <div className="mod-filters"><input className="mod-search" value={modQuery} onChange={e=>setModQuery(e.target.value)} onKeyDown={e=>e.key==="Enter"&&searchMods()} placeholder="Знайти Sodium, minimap, performance…"/>
            <div className="filter-row"><label>Версія гри<select value={modVersion} onChange={e=>{setModVersion(e.target.value);setModPage(0);}}>{versions.filter(v=>v.kind==="release").slice(0,80).map(v=><option key={v.id}>{v.id}</option>)}{!versions.length&&<option>1.21.4</option>}</select></label>
            <label>Завантажувач<select value={loader} onChange={e=>{setLoader(e.target.value);setModPage(0);}}><option value="fabric">Fabric</option><option value="forge">Forge</option><option value="neoforge">NeoForge</option><option value="quilt">Quilt</option><option value="vanilla">Vanilla / не фільтрувати</option></select></label>
            {modSource==="Modrinth"&&<label>Тип контенту<select value={projectType} onChange={e=>{setProjectType(e.target.value);setModPage(0);}}><option value="mod">Моди</option><option value="modpack">Збірки</option><option value="resourcepack">Ресурс-паки</option><option value="shader">Шейдери</option></select></label>}
            <label>Встановлювати в<select value={modTarget} onChange={e=>setModTarget(e.target.value)}><option value="">Основна папка гри</option>{profiles.map(p=><option key={p.id} value={p.id}>{p.name}</option>)}</select></label>
            <button className="play-button search-button" disabled={searching} onClick={searchMods}>{searching?"Шукаю…":"⌕  Шукати"}</button></div>
            {modSource==="CurseForge"&&<label className="api-key-label">CurseForge API key <input type="password" value={curseKey} onChange={e=>setCurseKey(e.target.value)} placeholder="Встав свій ключ API" onBlur={()=>setSearchTick(t=>t+1)} onKeyDown={e=>e.key==="Enter"&&setSearchTick(t=>t+1)}/></label>}
          </div>
          {modNotice&&<div className="notice wide-notice" role="status">{modNotice}</div>}
          {searching && !mods.length && <div className="empty-state"><div className="empty-icon">▦</div><p>Завантажую каталог…</p></div>}
          {modTarget && <div className="mod-bulk-bar"><span>Обрано модів: {selectedMods.length}</span><button className="play-button" disabled={!selectedMods.length || searching} onClick={async()=>{ const chosen=mods.filter(m=>selectedMods.includes(m.id)); for (const m of chosen) await installMod(m); setSelectedMods([]); }}>Завантажити вибрані в «{profiles.find(p=>p.id===modTarget)?.name || "збірку"}»</button></div>}
          <div className={`mod-results ${searching ? "loading" : ""}`}>{mods.map(mod=><article className="mod-card" key={`${mod.source}-${mod.id}`}><div className="mod-card-top">{mod.icon_url?<img className="mod-icon" src={mod.icon_url} alt="" />:<div className="mod-icon mod-icon-placeholder">◈</div>}<div className="mod-card-title"><strong>{mod.title}</strong><small>{mod.source} · від {mod.author}</small></div><span className="download-count">↓ {prettySize(mod.downloads)}</span></div><p>{mod.description||"Опис відсутній."}</p><div className="mod-card-bottom"><span>{mod.latest_file_name||"Сумісний файл за версією та loader"}</span><label className="mod-select-label"><input type="checkbox" checked={selectedMods.includes(mod.id)} onChange={e=>setSelectedMods(old=>e.target.checked?[...old,mod.id]:old.filter(id=>id!==mod.id))}/> До збірки</label><button className="install-mod-button" onClick={()=>installMod(mod)}>Завантажити ↓</button></div></article>)}</div>
          {modTotal > MOD_PAGE_SIZE && (() => { const totalPages = Math.min(400, Math.ceil(modTotal / MOD_PAGE_SIZE)); return <nav className="pager" aria-label="Сторінки">
            <button disabled={modPage === 0 || searching} onClick={() => setModPage(modPage - 1)}>←</button>
            {pageNumbers(modPage, totalPages).map((n, i) => n === "gap" ? <span key={`g${i}`} className="pager-gap">…</span> : <button key={n} className={n === modPage ? "selected" : ""} disabled={searching} onClick={() => setModPage(n)}>{n + 1}</button>)}
            <button disabled={modPage >= totalPages - 1 || searching} onClick={() => setModPage(modPage + 1)}>→</button>
            <span className="pager-info">{modTotal.toLocaleString("uk-UA")} результатів</span>
          </nav>; })()}
          <div className="info-banner"><span>ⓘ</span><p><strong>Сумісність важлива</strong><br/>Мод має відповідати версії Minecraft і завантажувачу. CurseForge вимагає власний API key; лаунчер не містить вбудованого секретного ключа.</p></div>
        </div>}

        {page === "skins" && <div className="page-content subpage">
          <div className="page-intro"><div className="eyebrow"><span className="eyebrow-line"/> ВИГЛЯД ПЕРСОНАЖА</div><h1>Скіни та <span>персонаж</span></h1><p>Додай PNG 64×64 або імпортуй скін за нікнеймом гравця. Обраний скін застосовується до всіх збірок при запуску.</p></div>
          <div className="settings-card"><div className="settings-card-icon">⇪</div><div className="settings-card-copy"><h3>Додати скін</h3><p>Файл PNG 64×64 (новий формат). Назва необов’язкова.</p>
            <div className="skin-add-row"><input value={skinName} maxLength={32} onChange={e=>setSkinName(e.target.value)} placeholder="Назва скіна"/>
            <label className="secondary-button file-button">Обрати PNG…<input type="file" accept="image/png" hidden onChange={e=>{uploadSkin(e.target.files?.[0]); e.target.value="";}}/></label></div></div></div>
          <div className="settings-card"><div className="settings-card-icon">☁</div><div className="settings-card-copy"><h3>Імпорт за нікнеймом</h3><p>Завантажує публічний скін ліцензійного гравця з серверів Mojang.</p>
            <div className="skin-add-row"><input value={skinUser} maxLength={16} onChange={e=>setSkinUser(e.target.value)} onKeyDown={e=>e.key==="Enter"&&importSkin()} placeholder="Нікнейм гравця"/><button className="play-button" disabled={skinBusy} onClick={importSkin}>{skinBusy?"Шукаю…":"Імпортувати"}</button></div></div></div>
          {skinNotice && <div className="notice wide-notice" role="status">{skinNotice}</div>}
          <div className="section-heading"><div><span className="section-kicker">БІБЛІОТЕКА</span><h2>Мої скіни</h2></div>{skins.some(k=>k.active)&&<button className="secondary-button" onClick={()=>activateSkin("")}>Вимкнути скін</button>}</div>
          <div className="skin-grid">{skins.map(k=><article key={k.name} className={`skin-card ${k.active?"active":""}`}>
            <SkinFigure src={k.data_url}/>
            <strong title={k.name}>{k.name}</strong>
            <div className="skin-actions"><button className={k.active?"secondary-button":"play-button"} disabled={k.active} onClick={()=>activateSkin(k.name)}>{k.active?"✓ Активний":"Використати"}</button><button className="secondary-button" onClick={()=>removeSkin(k.name)} aria-label="Видалити скін">🗑</button></div>
          </article>)}
          {!skins.length && <div className="empty-state"><div className="empty-icon">☺</div><h2>Скінів ще немає</h2><p>Додай PNG-файл або імпортуй скін за нікнеймом.</p></div>}</div>
          <div className="info-banner"><span>ⓘ</span><p><strong>Як це працює в офлайн-режимі</strong><br/>Без сервера Mojang гра не отримує скін автоматично, тому лаунчер підміняє стандартні скіни ресурс-паком. Скін бачите ви (одиночна гра та ваш клієнт); інші гравці на сервері бачитимуть стандартний.</p></div>
        </div>}

        {page === "settings" && <div className="page-content subpage">
          <div className="page-intro"><div className="eyebrow"><span className="eyebrow-line"/> JAVA RUNTIME</div><h1>Java та <span>налаштування</span></h1><p>Знайди встановлену Java або вкажи шлях до java.exe вручну.</p></div>
          <div className="settings-card"><div className="settings-card-icon">☕</div><div className="settings-card-copy"><h3>Шлях до Java</h3><p>Вибери знайдений runtime або введи повний шлях до java.exe. Java 21 зазвичай потрібна для Minecraft 1.20.5+, Java 17 для 1.18–1.20.4, а старі версії можуть вимагати Java 8.</p><input value={javaPath} onChange={e=>setJavaPath(e.target.value)} placeholder="C:\\Program Files\\Java\\...\\bin\\java.exe"/></div></div>
          <div className="java-save-row"><span>Вибраний шлях буде збережено в налаштуваннях лаунчера.</span><button className="play-button" onClick={async()=>{try{await invoke("save_java_path",{javaPath});setNotice("✓ Шлях до Java збережено.");}catch(e){setNotice(String(e));}}}>Зберегти Java</button></div>
          {notice && <div className="notice wide-notice" role="status">{notice}</div>}
          <div className="section-heading"><div><span className="section-kicker">ЗНАЙДЕНІ ВСТАНОВЛЕННЯ</span><h2>Доступні Java runtime</h2></div><button className="secondary-button" onClick={()=>invoke<JavaInfo[]>("detect_java").then(setJavaList).catch(e=>setNotice(String(e)))}>⟳ Оновити список</button></div>
          <div className="java-list">{javaList.map((j,i)=><button key={`${j.path}-${i}`} className={`java-row ${javaPath===j.path?"selected":""}`} onClick={()=>setJavaPath(j.path)}><span className="java-icon">☕</span><span className="version-meta"><strong>{j.version}</strong><small>{j.source}</small><code>{j.path}</code></span><span className="version-radio">{javaPath===j.path?"✓":"Вибрати"}</span></button>)}</div>
          {!javaList.length&&<div className="empty-state compact-empty"><h2>Java автоматично не знайдена</h2><p>Якщо Java встановлена, встав шлях до java.exe вище. Вибраний шлях поки зберігається лише в інтерфейсі; запуск гри підключимо окремо.</p></div>}
          <div className="info-banner"><span>ⓘ</span><p><strong>Важливо</strong><br/>Поки що шлях можна вибрати й перевірити в інтерфейсі. Перед запуском Minecraft наступним кроком збережемо його в налаштуваннях профілю та передаватимемо процесу гри.</p></div>
        </div>}
      </section>
    </main>
  );
}
export default App;
