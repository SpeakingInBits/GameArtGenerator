// Game Art Generator UI. Talks to the Rust backend (src-tauri/src/lib.rs) through Tauri commands.
"use strict";

const { invoke, convertFileSrc } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const { open: openDialog } = window.__TAURI__.dialog;
const { getCurrentWebview } = window.__TAURI__.webview;

const $ = (id) => document.getElementById(id);
const IMAGE_EXTENSIONS = ["png", "jpg", "jpeg", "webp", "bmp", "gif"];

const state = {
  info: null,
  settings: null,
  models: [],
  references: [], // { path, name, width, height }
  history: [],
  selectedId: null,
  running: 0,
  results: [], // { path, caption } newest first
  lightbox: { list: [], index: 0 },
  menuPath: null,
  update: { state: "idle" },
};

// ---- helpers ------------------------------------------------------------------------------------------------------

const money = (x) => (x == null ? "—" : `$${x.toFixed(3)}`);
const plural = (n, word) => `${n.toLocaleString()} ${word}${n === 1 ? "" : "s"}`;
const fileName = (p) => p.split(/[\\/]/).pop();
const imgSrc = (p) => convertFileSrc(p);

function el(tag, attrs = {}, ...children) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") e.className = v;
    else if (k === "text") e.textContent = v;
    else if (k.startsWith("on")) e.addEventListener(k.slice(2), v);
    else if (v !== undefined && v !== null && v !== false) e.setAttribute(k, v === true ? "" : v);
  }
  for (const c of children) if (c) e.append(c);
  return e;
}

function toast(message, kind = "info", ms = 6000) {
  const t = el("div", { class: `toast ${kind}`, text: String(message) });
  $("toasts").append(t);
  setTimeout(() => t.remove(), kind === "error" ? Math.max(ms, 12000) : ms);
}

function formatWhen(iso) {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString(undefined, { year: "numeric", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });
}

function debounce(fn, ms) {
  let t;
  return (...args) => {
    clearTimeout(t);
    t = setTimeout(() => fn(...args), ms);
  };
}

// ---- tabs ---------------------------------------------------------------------------------------------------------

function showTab(name) {
  document.querySelectorAll(".tab").forEach((b) => b.classList.toggle("active", b.dataset.tab === name));
  document.querySelectorAll(".tab-panel").forEach((p) => p.classList.toggle("active", p.id === `tab-${name}`));
  if (name === "history") renderHistory();
}
document.querySelectorAll(".tab").forEach((b) => b.addEventListener("click", () => showTab(b.dataset.tab)));

// ---- models & settings controls -----------------------------------------------------------------------------------

const currentModel = () => state.models.find((m) => m.id === $("model").value) || state.models[0];

function priceLabel(m) {
  if (m.pricing.length) {
    const prices = m.pricing.map((p) => p.priceUsd);
    return `$${Math.min(...prices).toFixed(2)}–${Math.max(...prices).toFixed(2)}`;
  }
  return `$${m.imagePriceUsd.toFixed(2)}`;
}

function setModels(models, select) {
  state.models = models;
  const sel = $("model");
  sel.replaceChildren(...models.map((m) => el("option", { value: m.id, text: `${m.id} (${priceLabel(m)})` })));
  if (select && models.some((m) => m.id === select)) sel.value = select;
  modelChanged();
}

function modelChanged() {
  const m = currentModel();
  if (!m) return;
  const res = $("resolution");
  const keep = res.value || state.settings?.resolution || "1k";
  res.replaceChildren(...m.resolutions.map((r) => el("option", { value: r, text: r })));
  res.value = m.resolutions.includes(keep) ? keep : m.resolutions[0];
  $("quality").disabled = !m.supportsQuality;
  $("quality-hint").textContent = m.supportsQuality
    ? "Quality auto = low for new images, medium when reference images are used."
    : "This model has a single quality level.";
  settingsChanged();
}

function currentParams() {
  const m = currentModel();
  return {
    model: m?.id ?? "",
    prompt: $("prompt").value,
    n: Math.min(10, Math.max(1, parseInt($("count").value, 10) || 1)),
    aspectRatio: $("aspect").value,
    resolution: $("resolution").value,
    quality: m?.supportsQuality ? $("quality").value : "standard",
  };
}

const saveSettings = debounce(saveSettingsNow, 600);
async function saveSettingsNow() {
  if (!state.settings) return;
  const p = currentParams();
  const settings = {
    ...state.settings,
    model: p.model,
    n: p.n,
    aspectRatio: p.aspectRatio,
    resolution: p.resolution,
    quality: $("quality").value,
    prompt: p.prompt,
  };
  try {
    await invoke("save_settings", { settings });
    state.settings = settings;
  } catch (e) {
    console.error(e);
  }
}

async function updateEstimate() {
  const m = currentModel();
  if (!m) return;
  const p = currentParams();
  $("prompt-count").textContent = `${p.prompt.length.toLocaleString()} / ${m.maxPromptLength.toLocaleString()}`;
  try {
    const est = await invoke("estimate_cost", { params: p, references: state.references.length });
    $("estimate-total").textContent = money(est.totalUsd);
    let detail = `${p.n} × ${money(est.perImageUsd)}`;
    if (state.references.length) detail += ` + ${plural(state.references.length, "reference")} (${money(est.referencesUsd)})`;
    $("estimate-detail").textContent = detail;
  } catch (e) {
    $("estimate-total").textContent = "—";
    $("estimate-detail").textContent = String(e);
  }
}

function settingsChanged() {
  updateEstimate();
  saveSettings();
}

["count", "aspect", "resolution", "quality"].forEach((id) => $(id).addEventListener("change", settingsChanged));
$("count").addEventListener("input", settingsChanged);
$("model").addEventListener("change", modelChanged);
$("prompt").addEventListener("input", settingsChanged);

// ---- reference images ---------------------------------------------------------------------------------------------

function renderReferences() {
  const max = state.info?.maxReferenceImages ?? 5;
  const n = state.references.length;
  $("ref-count").textContent = `(${n}/${max})`;
  $("ref-mode").textContent = n ? "Sent in this order: image 1, image 2…" : "";
  $("ref-empty").hidden = n > 0;
  $("ref-grid").replaceChildren(
    ...state.references.map((r, i) =>
      el(
        "div",
        { class: "ref-item", title: `${r.name} (${r.width}×${r.height})\n${r.path}` },
        el("img", { src: imgSrc(r.path), alt: r.name, draggable: "false" }),
        el("span", { class: "ref-num", text: String(i + 1) }),
        el("button", {
          class: "ref-remove",
          title: "Remove",
          text: "✕",
          onclick: () => {
            state.references.splice(i, 1);
            renderReferences();
          },
        }),
      ),
    ),
  );
  updateEstimate();
}

function addReferenceInfo(info) {
  const max = state.info?.maxReferenceImages ?? 5;
  if (state.references.some((r) => r.path.toLowerCase() === info.path.toLowerCase())) return true;
  if (state.references.length >= max) {
    toast(`Only ${max} reference images can be sent per request.`, "error");
    return false;
  }
  state.references.push(info);
  return true;
}

async function addReferencePaths(paths) {
  for (const path of paths) {
    const ext = path.split(".").pop().toLowerCase();
    if (!IMAGE_EXTENSIONS.includes(ext)) {
      toast(`${fileName(path)} isn't a supported image (PNG, JPEG, WebP, BMP or GIF).`, "error");
      continue;
    }
    try {
      if (!addReferenceInfo(await invoke("add_reference", { path }))) break;
    } catch (e) {
      toast(e, "error");
    }
  }
  renderReferences();
}

async function pasteBytes(file) {
  const bytes = new Uint8Array(await file.arrayBuffer());
  const info = await invoke("save_pasted_image", bytes);
  addReferenceInfo(info);
  renderReferences();
}

$("ref-add").addEventListener("click", async () => {
  const picked = await openDialog({
    multiple: true,
    title: "Add reference images",
    filters: [{ name: "Images", extensions: IMAGE_EXTENSIONS }],
  });
  if (picked) addReferencePaths(Array.isArray(picked) ? picked : [picked]);
});

$("ref-paste").addEventListener("click", async () => {
  try {
    addReferenceInfo(await invoke("paste_reference"));
    renderReferences();
  } catch (e) {
    toast(e, "error");
  }
});

$("ref-clear").addEventListener("click", () => {
  state.references = [];
  renderReferences();
});

document.addEventListener("paste", async (e) => {
  if (!$("tab-generate").classList.contains("active")) return;
  const files = [...(e.clipboardData?.files ?? [])].filter((f) => f.type.startsWith("image/"));
  if (!files.length) return; // plain text: let the text box handle it
  e.preventDefault();
  for (const f of files) {
    try {
      await pasteBytes(f);
    } catch (err) {
      toast(err, "error");
    }
  }
});

// Files dragged in from Explorer. Tauri reports their paths.
getCurrentWebview().onDragDropEvent((event) => {
  const p = event.payload;
  if (p.type === "enter") {
    $("drop-overlay").hidden = false;
  } else if (p.type === "leave") {
    $("drop-overlay").hidden = true;
  } else if (p.type === "drop") {
    $("drop-overlay").hidden = true;
    showTab("generate");
    addReferencePaths(p.paths || []);
  }
});

// ---- generating ---------------------------------------------------------------------------------------------------

function updateGenerateButton() {
  const label = state.running ? `Generate another <span class="kbd-hint">${state.running} running</span>` : `Generate <span class="kbd-hint">Ctrl+Enter</span>`;
  $("generate").innerHTML = label;
}

async function generate() {
  if (state.info?.apiKeySource === "none") {
    openSettings();
    toast("Add your xAI API key first.", "error");
    return;
  }
  const params = currentParams();
  if (!params.prompt.trim()) {
    toast("Enter a prompt first.", "error");
    $("prompt").focus();
    return;
  }
  const references = state.references.map((r) => r.path);
  saveSettingsNow();

  $("results-empty").hidden = true;
  const pending = el(
    "div",
    { class: "card pending" },
    el("div", {}, el("div", { class: "spinner" }), el("p", { text: `Generating ${plural(params.n, "image")}…` })),
  );
  $("results").insertBefore(pending, $("results-empty").nextSibling);

  state.running++;
  updateGenerateButton();
  try {
    const rec = await invoke("generate", { params, references });
    const caption = `${rec.prompt.slice(0, 120)} · ${money(rec.costUsd)}`;
    const entries = rec.images.map((img) => ({ path: img.path, caption }));
    state.results.unshift(...entries);
    const cards = entries.map((e) => resultCard(e, () => state.results));
    pending.replaceWith(...cards);
    toast(`Generated ${plural(rec.images.length, "image")} in ${rec.durationS}s for ${money(rec.costUsd)}.`, "ok");
  } catch (e) {
    pending.remove();
    if (!state.results.length) $("results-empty").hidden = false;
    toast(`Generation failed: ${e}`, "error");
  } finally {
    state.running--;
    updateGenerateButton();
    refreshCosts();
    loadHistory();
  }
}

function resultCard(entry, listFn, badge) {
  const card = el(
    "div",
    {
      class: "card",
      title: entry.caption,
      onclick: () => {
        const list = listFn();
        openLightbox(list, Math.max(0, list.findIndex((x) => x.path === entry.path)));
      },
      oncontextmenu: (e) => showMenu(e, entry.path),
    },
    el("img", { src: imgSrc(entry.path), alt: "", loading: "lazy", draggable: "false" }),
    badge ? el("span", { class: "badge", text: badge }) : null,
    el("div", { class: "card-meta", text: entry.caption }),
  );
  return card;
}

$("generate").addEventListener("click", generate);
$("results-clear").addEventListener("click", () => {
  state.results = [];
  $("results").querySelectorAll(".card:not(.pending)").forEach((c) => c.remove());
  $("results-empty").hidden = $("results").querySelector(".card") !== null;
});

// ---- costs --------------------------------------------------------------------------------------------------------

async function refreshCosts() {
  try {
    const c = await invoke("get_costs");
    $("cost-last").textContent = money(c.lastUsd);
    $("cost-session").textContent = money(c.sessionUsd);
    $("cost-session-count").textContent = plural(c.sessionCount, "generation");
    $("cost-total").textContent = money(c.allTime.costUsd);
    $("cost-total-count").textContent = `${plural(c.allTime.succeeded, "generation")} · ${plural(c.allTime.images, "image")}`;
    state.allTime = c.allTime;
    renderHistorySummary();
  } catch (e) {
    console.error(e);
  }
}

// ---- history ------------------------------------------------------------------------------------------------------

async function loadHistory() {
  refreshCosts();
  try {
    state.history = await invoke("list_history");
  } catch (e) {
    toast(`Couldn't load the history: ${e}`, "error");
  }
  if ($("tab-history").classList.contains("active")) renderHistory();
}

function renderHistorySummary() {
  const t = state.allTime;
  if (!t) return;
  const avg = t.succeeded ? t.costUsd / t.succeeded : 0;
  $("history-summary").textContent =
    `${plural(t.generations, "generation")} (${t.succeeded} succeeded, ${t.failed} failed) · ` +
    `${plural(t.images, "image")} · ${money(t.costUsd)} spent · ${money(avg)} average`;
}

function renderHistory() {
  const q = $("history-search").value.trim().toLowerCase();
  const rows = state.history.filter((r) => !q || r.prompt.toLowerCase().includes(q));
  $("history-empty").hidden = rows.length > 0;
  $("history-empty").querySelector("p").textContent = state.history.length ? "No prompts match." : "No generations yet.";
  const tbody = $("history-table").querySelector("tbody");
  tbody.replaceChildren(
    ...rows.map((r) =>
      el(
        "tr",
        { class: r.id === state.selectedId ? "selected" : "", onclick: () => selectHistory(r.id) },
        el("td", { text: formatWhen(r.createdAt) }),
        el("td", { class: "prompt", text: r.prompt, title: r.prompt }),
        el("td", { text: r.model }),
        el("td", { class: "num", text: String(r.referenceImages.length) }),
        el("td", { class: "num", text: String(r.images.length) }),
        el("td", { class: "num", text: money(r.costUsd) }),
        el("td", {
          class: r.status === "ok" ? "status-ok" : "status-error",
          text: r.status === "ok" ? "OK" : "Failed",
          title: r.error || "",
        }),
      ),
    ),
  );
  renderHistorySummary();
  renderHistoryDetail();
}

function selectHistory(id) {
  state.selectedId = id;
  renderHistory();
}

function renderHistoryDetail() {
  const box = $("history-detail");
  const r = state.history.find((x) => x.id === state.selectedId);
  if (!r) {
    box.replaceChildren(el("p", { class: "muted", text: "Select a generation to see its images and details." }));
    return;
  }
  const quality = r.quality === "standard" ? "" : ` · ${r.quality} quality`;
  let costText = money(r.costUsd);
  if (r.costSource === "api") costText += ` (estimate was ${money(r.estimatedCostUsd)})`;
  if (r.costSource === "estimate") costText += " (estimated; the API didn't report a cost)";
  if (r.costSource === "none") costText += " (not charged)";

  const all = [
    ...r.referenceImages.map((f, i) => ({ ...f, caption: `Reference ${i + 1}`, badge: `REF ${i + 1}` })),
    ...r.images.map((f, i) => ({ ...f, caption: `Output ${i + 1} · ${r.prompt.slice(0, 100)}` })),
  ];
  const viewable = all.filter((f) => f.exists);
  const tile = (f) =>
    f.exists
      ? resultCard(f, () => viewable, f.badge)
      : el("div", { class: "card missing", title: f.path, text: "File missing" });

  const facts = el(
    "dl",
    { class: "detail-facts" },
    el("dt", { text: "When" }), el("dd", { text: formatWhen(r.createdAt) }),
    el("dt", { text: "Model" }), el("dd", { text: r.model }),
    el("dt", { text: "Settings" }), el("dd", { text: `${plural(r.n, "image")} · ${r.aspectRatio} · ${r.resolution}${quality}` }),
    el("dt", { text: "Cost" }), el("dd", { text: costText }),
    el("dt", { text: "Took" }), el("dd", { text: `${r.durationS}s` }),
  );

  const outFolder = r.images.find((f) => f.exists)?.path;
  const parts = [
    el("div", { class: "detail-prompt", text: r.prompt }),
    r.error ? el("div", { class: "detail-error", text: r.error }) : null,
    facts,
    r.referenceImages.length ? el("h3", { text: `Reference images (${r.referenceImages.length})` }) : null,
    r.referenceImages.length ? el("div", { class: "detail-gallery" }, ...all.filter((f) => f.badge).map(tile)) : null,
    r.images.length ? el("h3", { text: `Outputs (${r.images.length})` }) : null,
    r.images.length ? el("div", { class: "detail-gallery" }, ...all.filter((f) => !f.badge).map(tile)) : null,
    el(
      "div",
      { class: "detail-actions" },
      el("button", { class: "button primary", text: "Reuse prompt, settings & references", onclick: () => reuse(r) }),
      outFolder ? el("button", { class: "button", text: "Show in folder", onclick: () => invoke("show_in_folder", { path: outFolder }) }) : null,
      el("button", {
        class: "button",
        text: "Copy prompt",
        onclick: async () => {
          await navigator.clipboard.writeText(r.prompt);
          toast("Prompt copied.", "ok", 2500);
        },
      }),
    ),
  ];
  box.replaceChildren(...parts.filter(Boolean));
}

async function reuse(r) {
  $("prompt").value = r.prompt;
  if (state.models.some((m) => m.id === r.model)) {
    $("model").value = r.model;
    modelChanged();
  }
  $("count").value = r.n;
  $("aspect").value = r.aspectRatio;
  if ([...$("resolution").options].some((o) => o.value === r.resolution)) $("resolution").value = r.resolution;
  if (r.quality !== "standard") $("quality").value = r.quality;
  state.references = [];
  const missing = r.referenceImages.filter((f) => !f.exists).length;
  await addReferencePaths(r.referenceImages.filter((f) => f.exists).map((f) => f.path));
  if (missing) toast(`${plural(missing, "reference image")} from that generation no longer exist.`, "error");
  settingsChanged();
  showTab("generate");
}

$("history-search").addEventListener("input", renderHistory);

// ---- lightbox & context menu --------------------------------------------------------------------------------------

function openLightbox(list, index) {
  if (!list.length) return;
  state.lightbox = { list, index };
  renderLightbox();
  $("lightbox").hidden = false;
}

function renderLightbox() {
  const { list, index } = state.lightbox;
  const item = list[index];
  $("lightbox-img").src = imgSrc(item.path);
  $("lightbox-caption").textContent = `${index + 1} / ${list.length} · ${item.caption || fileName(item.path)}`;
  document.querySelectorAll(".lightbox-nav").forEach((b) => (b.hidden = list.length < 2));
}

function stepLightbox(delta) {
  const { list } = state.lightbox;
  state.lightbox.index = (state.lightbox.index + delta + list.length) % list.length;
  renderLightbox();
}

const closeLightbox = () => ($("lightbox").hidden = true);
$("lightbox").addEventListener("click", (e) => {
  if (e.target.id === "lightbox") closeLightbox();
});
document.querySelector(".lightbox-close").addEventListener("click", closeLightbox);
document.querySelector(".lightbox-nav.prev").addEventListener("click", () => stepLightbox(-1));
document.querySelector(".lightbox-nav.next").addEventListener("click", () => stepLightbox(1));
$("lightbox-img").addEventListener("contextmenu", (e) => showMenu(e, state.lightbox.list[state.lightbox.index].path));
document.querySelectorAll(".lightbox-bar [data-action]").forEach((b) =>
  b.addEventListener("click", () => imageAction(b.dataset.action, state.lightbox.list[state.lightbox.index].path)),
);

function showMenu(e, path) {
  e.preventDefault();
  e.stopPropagation();
  state.menuPath = path;
  const menu = $("context-menu");
  menu.hidden = false;
  const { innerWidth: w, innerHeight: h } = window;
  const r = menu.getBoundingClientRect();
  menu.style.left = `${Math.min(e.clientX, w - r.width - 4)}px`;
  menu.style.top = `${Math.min(e.clientY, h - r.height - 4)}px`;
}
const hideMenu = () => ($("context-menu").hidden = true);
document.addEventListener("click", hideMenu);
document.addEventListener("contextmenu", (e) => {
  // Allow the normal menu in text fields (copy/paste); hide the browser menu elsewhere.
  if (!e.target.closest("textarea, input")) e.preventDefault();
  hideMenu();
});
document.querySelectorAll("#context-menu [data-action]").forEach((b) =>
  b.addEventListener("click", () => imageAction(b.dataset.action, state.menuPath)),
);

async function imageAction(action, path) {
  hideMenu();
  try {
    if (action === "open") await invoke("open_path", { path });
    if (action === "folder") await invoke("show_in_folder", { path });
    if (action === "copy") {
      await invoke("copy_image", { path });
      toast("Image copied to the clipboard.", "ok", 2500);
    }
    if (action === "reference") {
      closeLightbox();
      await addReferencePaths([path]);
      showTab("generate");
    }
  } catch (e) {
    toast(e, "error");
  }
}

// ---- settings dialog ----------------------------------------------------------------------------------------------

function renderSettingsDialog() {
  const info = state.info;
  const keyText = {
    environment: "Using the XAI_API_KEY environment variable. It takes priority over a key saved here.",
    credentialManager: "A key is saved. Enter a new one to replace it.",
    none: "No key is set.",
  };
  $("key-status").textContent = keyText[info.apiKeySource] ?? "";
  $("key-remove").disabled = info.apiKeySource !== "credentialManager";
  $("images-dir").value = info.imagesDir;
  $("images-dir-reset").disabled = !state.settings?.imagesDir;
  $("data-dir").value = info.dataDir;
  $("data-mode").textContent = {
    installed: "Your history, settings and (by default) images. Kept outside the app's install folder, so updating or uninstalling the app doesn't delete them.",
    portable: "Portable copy: your history, settings and images are kept in the app's own folder, so you can move or copy the whole folder.",
    development: "Development build.",
  }[info.mode];
  $("version-text").textContent = `Version ${info.version}`;
  renderUpdateStatus();
}

function openSettings() {
  renderSettingsDialog();
  $("settings-dialog").showModal();
}

$("settings-button").addEventListener("click", openSettings);
$("key-banner-button").addEventListener("click", openSettings);

async function refreshInfo() {
  state.info = await invoke("get_app_info");
  $("key-banner").hidden = state.info.apiKeySource !== "none";
}

$("key-save").addEventListener("click", async () => {
  const key = $("key-input").value.trim();
  if (!key) return;
  try {
    await invoke("set_api_key", { key });
    $("key-input").value = "";
    await refreshInfo();
    renderSettingsDialog();
    toast("API key saved.", "ok", 3000);
    loadModels();
  } catch (e) {
    toast(e, "error");
  }
});

$("key-remove").addEventListener("click", async () => {
  try {
    await invoke("set_api_key", { key: "" });
    await refreshInfo();
    renderSettingsDialog();
    toast("Saved API key removed.", "ok", 3000);
  } catch (e) {
    toast(e, "error");
  }
});

async function setImagesDir(dir) {
  try {
    const settings = { ...state.settings, imagesDir: dir };
    await invoke("save_settings", { settings });
    state.settings = settings;
    await refreshInfo();
    renderSettingsDialog();
  } catch (e) {
    toast(e, "error");
  }
}

$("images-dir-change").addEventListener("click", async () => {
  const dir = await openDialog({ directory: true, title: "Folder for new images", defaultPath: state.info.imagesDir });
  if (dir) setImagesDir(dir);
});
$("images-dir-reset").addEventListener("click", () => setImagesDir(null));
$("data-dir-open").addEventListener("click", () => invoke("open_path", { path: state.info.dataDir }));
document.querySelectorAll("[data-open]").forEach((a) =>
  a.addEventListener("click", (e) => {
    e.preventDefault();
    invoke("open_path", { path: a.dataset.open });
  }),
);

// ---- updates ------------------------------------------------------------------------------------------------------

function renderUpdateStatus() {
  const u = state.update;
  const text = {
    notInstalled: "Updates work in installed copies only.",
    idle: "",
    checking: "Checking for updates…",
    upToDate: "Up to date.",
    downloading: `Downloading version ${u.version}…`,
    ready: `Version ${u.version} is ready.`,
    failed: u.message,
  }[u.state];
  $("update-status").textContent = text ?? "";
  $("update-check").disabled = ["notInstalled", "checking", "downloading"].includes(u.state);
  const ready = u.state === "ready";
  $("update-banner").hidden = !ready;
  $("settings-dot").hidden = !ready;
  if (ready) $("update-text").textContent = `Version ${u.version} is ready. It installs the next time you open the app.`;
}

$("update-check").addEventListener("click", () => invoke("check_for_updates"));
$("update-restart").addEventListener("click", async () => {
  if (state.running && !confirm("Generations are still running. Their images won't be saved if you restart now. Restart anyway?")) return;
  await saveSettingsNow();
  try {
    await invoke("restart_to_update");
  } catch (e) {
    toast(e, "error");
  }
});

// ---- keyboard -----------------------------------------------------------------------------------------------------

document.addEventListener("keydown", (e) => {
  if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) {
    e.preventDefault();
    if ($("tab-generate").classList.contains("active")) generate();
  } else if (!$("lightbox").hidden) {
    if (e.key === "Escape") closeLightbox();
    if (e.key === "ArrowLeft") stepLightbox(-1);
    if (e.key === "ArrowRight") stepLightbox(1);
  } else if (e.key === "Escape") {
    hideMenu();
  }
});

// ---- startup ------------------------------------------------------------------------------------------------------

async function loadModels() {
  const select = $("model").value || state.settings?.model;
  try {
    const list = await invoke("list_models");
    setModels(list.models, select);
    if (!list.live && state.info.apiKeySource !== "none") {
      toast(`Using built-in prices; couldn't get the current ones from xAI: ${list.error}`, "error");
    }
  } catch (e) {
    toast(e, "error");
  }
}

async function init() {
  await refreshInfo();
  state.settings = await invoke("get_settings");
  const s = state.settings;

  $("aspect").replaceChildren(...state.info.aspectRatios.map((a) => el("option", { value: a, text: a })));
  $("aspect").value = s.aspectRatio;
  $("count").value = s.n;
  $("quality").value = s.quality;
  $("prompt").value = s.prompt;
  renderReferences();
  updateGenerateButton();

  state.update = await invoke("get_update_status");
  renderUpdateStatus();
  listen("update-status", (e) => {
    state.update = e.payload;
    renderUpdateStatus();
  });

  refreshCosts();
  loadHistory();
  await loadModels();
  $("prompt").focus();
}

init().catch((e) => toast(`Startup failed: ${e}`, "error"));
