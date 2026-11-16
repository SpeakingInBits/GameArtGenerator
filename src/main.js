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
  results: [], // { path, caption, model? } newest first; `model` is a 3D model file shown by its preview
  modelJobs: [], // 3D model jobs (Tripo), newest first
  lightbox: { list: [], index: 0 },
  menuPath: null,
  menuModel: null,
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
  if (name === "audio") renderAudioUsage();
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
      oncontextmenu: (e) => showMenu(e, entry.path, entry.model),
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
    $("cost-total-count").textContent =
      `${plural(c.allTime.succeeded, "generation")} · ${plural(c.allTime.images, "image")}` +
      (c.allTime.models ? ` · ${plural(c.allTime.models, "3D model")}` : "") +
      (c.allTime.audioClips ? ` · ${plural(c.allTime.audioClips, "sound")}` : "");
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
    [state.history, state.modelJobs] = await Promise.all([invoke("list_history"), invoke("list_model_jobs")]);
  } catch (e) {
    toast(`Couldn't load the history: ${e}`, "error");
  }
  if ($("tab-history").classList.contains("active")) renderHistory();
}

function renderHistorySummary() {
  const t = state.allTime;
  if (!t) return;
  const avg = t.succeeded ? (t.costUsd - modelSpend()) / t.succeeded : 0;
  $("history-summary").textContent =
    `${plural(t.generations, "generation")} (${t.succeeded} succeeded, ${t.failed} failed) · ` +
    `${plural(t.images, "image")} · ` +
    (t.models ? `${plural(t.models, "3D model")} · ` : "") +
    `${money(t.costUsd)} spent · ${money(avg)} average per generation`;
}

const modelSpend = () => state.modelJobs.reduce((sum, j) => sum + j.costUsd, 0);

const RIG_TYPE_NAMES = {
  biped: "humanoid",
  quadruped: "four-legged",
  hexapod: "six-legged",
  octopod: "eight-legged",
  avian: "winged",
  serpentine: "snake-like",
  aquatic: "aquatic",
  others: "other",
};
const SERVICE_NAMES = { model: "Tripo 3D", rig: "Tripo rig", animate: "Tripo animation" };
// "preset:quadruped:walk" -> "walk"; Tripo calls the snake and fish move cycle "march".
const animationName = (id) => {
  const last = id.split(":").pop().replace(/_/g, " ");
  return last === "march" ? "move" : last;
};

// The model job a rig or animate job builds on (or the job itself for a model).
function rootModelJob(j) {
  let cur = j;
  for (let i = 0; cur && cur.kind !== "model" && i < 5; i++) cur = state.modelJobs.find((x) => x.id === cur.parentId);
  return cur?.kind === "model" ? cur : null;
}

// What a 3D model job shows as its "prompt" in the History table.
function modelJobLabel(j) {
  const root = rootModelJob(j);
  const from = `the 3D model from ${fileName((root ?? j).source.path)}`;
  const o = j.options || {};
  if (j.kind === "rig") {
    const spec = o.rigType === "biped" ? `, ${o.spec} skeleton` : "";
    return `Rig (${RIG_TYPE_NAMES[o.rigType] ?? o.rigType}${spec}) for ${from}`;
  }
  if (j.kind === "animate") return `Animations (${(o.animations || []).map(animationName).join(", ")}) for ${from}`;
  const what = j.inputs.length === 1 && j.inputs[0].view === "image" ? "whole image" : `${j.inputs.length} views`;
  return `3D model from ${fileName(j.source.path)} (${what})`;
}

// The finished model job whose .glb is at `path`, for offering "Rig & animate…".
const riggableJobFor = (path) =>
  path ? state.modelJobs.find((j) => j.kind === "model" && j.status === "ok" && j.model?.path === path) : null;

function historyRows() {
  const images = state.history.map((r) => ({
    id: r.id,
    createdAt: r.createdAt,
    label: r.prompt,
    model: r.model,
    refs: r.referenceImages.length,
    outputs: r.images.length,
    costUsd: r.costUsd,
    status: r.status,
    progress: 100,
    error: r.error,
  }));
  const models = state.modelJobs.map((j) => ({
    id: j.id,
    createdAt: j.createdAt,
    label: modelJobLabel(j),
    model: SERVICE_NAMES[j.kind] ?? "Tripo",
    refs: j.inputs.length,
    outputs: j.model?.exists ? 1 : 0,
    costUsd: j.costUsd,
    status: j.status,
    progress: j.progress,
    error: j.error,
  }));
  return [...images, ...models].sort((a, b) => (a.createdAt < b.createdAt ? 1 : a.createdAt > b.createdAt ? -1 : 0));
}

function renderHistory() {
  const q = $("history-search").value.trim().toLowerCase();
  const all = historyRows();
  const rows = all.filter((r) => !q || r.label.toLowerCase().includes(q));
  $("history-empty").hidden = rows.length > 0;
  $("history-empty").querySelector("p").textContent = all.length ? "No prompts match." : "No generations yet.";
  const statusCell = (r) => {
    if (r.status === "running") return el("td", { class: "status-running", text: `Running ${r.progress}%` });
    return el("td", {
      class: r.status === "ok" ? "status-ok" : "status-error",
      text: r.status === "ok" ? "OK" : "Failed",
      title: r.error || "",
    });
  };
  const tbody = $("history-table").querySelector("tbody");
  tbody.replaceChildren(
    ...rows.map((r) =>
      el(
        "tr",
        { class: r.id === state.selectedId ? "selected" : "", onclick: () => selectHistory(r.id) },
        el("td", { text: formatWhen(r.createdAt) }),
        el("td", { class: "prompt", text: r.label, title: r.label }),
        el("td", { text: r.model }),
        el("td", { class: "num", text: String(r.refs) }),
        el("td", { class: "num", text: String(r.outputs) }),
        el("td", { class: "num", text: money(r.costUsd) }),
        statusCell(r),
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

function renderRigJobDetail(j) {
  const o = j.options || {};
  let costText = money(j.costUsd);
  if (j.credits != null) costText += ` (${j.credits} credits; estimate was ${money(j.estimatedCostUsd)})`;
  else if (j.status === "running") costText = `about ${money(j.estimatedCostUsd)} (charged when it finishes)`;
  else costText += " (not charged)";
  const root = rootModelJob(j);
  const model = j.model?.exists ? j.model.path : null;
  const preview = root?.preview?.exists ? { path: root.preview.path, caption: modelJobLabel(j), model } : null;
  const facts = el(
    "dl",
    { class: "detail-facts" },
    el("dt", { text: "When" }), el("dd", { text: formatWhen(j.createdAt) }),
    el("dt", { text: "Service" }), el("dd", { text: SERVICE_NAMES[j.kind] }),
    el("dt", { text: "Body type" }), el("dd", { text: RIG_TYPE_NAMES[o.rigType] ?? o.rigType ?? "—" }),
    o.rigType === "biped" ? el("dt", { text: "Skeleton" }) : null,
    o.rigType === "biped" ? el("dd", { text: o.spec }) : null,
    el("dt", { text: "Animations" }),
    el("dd", { text: o.animations?.length ? o.animations.map(animationName).join(", ") + (o.inPlace ? " (in place)" : "") : "none" }),
    el("dt", { text: "Format" }), el("dd", { text: (o.format || "glb").toUpperCase() }),
    el("dt", { text: "Cost" }), el("dd", { text: costText }),
    el("dt", { text: j.status === "running" ? "Progress" : "Took" }),
    el("dd", { text: j.status === "running" ? `${j.progress}%` : `${j.durationS}s` }),
    el("dt", { text: "Task" }), el("dd", { text: j.taskId || "—" }),
  );
  const parts = [
    el("div", { class: "detail-prompt", text: modelJobLabel(j) }),
    j.error ? el("div", { class: "detail-error", text: j.error }) : null,
    facts,
    preview ? el("h3", { text: "Model" }) : null,
    preview ? el("div", { class: "detail-gallery" }, resultCard(preview, () => [preview], j.kind === "rig" ? "RIG" : "ANIM")) : null,
    el(
      "div",
      { class: "detail-actions" },
      model
        ? el("button", {
            class: "button primary",
            text: j.kind === "rig" ? "Open rigged model" : "Open animated model",
            onclick: () => imageAction("openModel", null, model),
          })
        : null,
      el("button", { class: "button", text: "Show in folder", onclick: () => (model ? invoke("show_in_folder", { path: model }) : invoke("open_path", { path: j.folder })) }),
      root?.status === "ok" ? el("button", { class: "button", text: "Rig again…", onclick: () => openRigDialog(root.id) }) : null,
    ),
  ];
  $("history-detail").replaceChildren(...parts.filter(Boolean));
}

function renderModelJobDetail(j) {
  if (j.kind !== "model") {
    renderRigJobDetail(j);
    return;
  }
  const opts = j.options || {};
  const texture = { none: "no texture", standard: "standard texture", detailed: "detailed texture" }[opts.texture] ?? opts.texture;
  const optionText = [texture, opts.texture !== "none" && opts.pbr ? "PBR" : null, opts.lowPoly ? "low-poly" : null]
    .filter(Boolean)
    .join(" · ");
  let costText = money(j.costUsd);
  if (j.credits != null) costText += ` (${j.credits} credits; estimate was ${money(j.estimatedCostUsd)})`;
  else if (j.status === "running") costText = `about ${money(j.estimatedCostUsd)} (charged when it finishes)`;
  else costText += " (not charged)";

  const inputs = j.inputs.map((i) => ({
    path: i.path,
    exists: i.exists,
    caption: i.view === "image" ? "Image sent to Tripo" : `${i.view} view`,
    badge: i.view === "image" ? "INPUT" : i.view.toUpperCase(),
  }));
  const viewable = inputs.filter((f) => f.exists);
  const tile = (f) =>
    f.exists ? resultCard(f, () => viewable, f.badge) : el("div", { class: "card missing", title: f.path, text: "File missing" });

  const facts = el(
    "dl",
    { class: "detail-facts" },
    el("dt", { text: "When" }), el("dd", { text: formatWhen(j.createdAt) }),
    el("dt", { text: "Service" }), el("dd", { text: "Tripo 3D" }),
    el("dt", { text: "Options" }), el("dd", { text: optionText }),
    el("dt", { text: "Cost" }), el("dd", { text: costText }),
    el("dt", { text: j.status === "running" ? "Progress" : "Took" }),
    el("dd", { text: j.status === "running" ? `${j.progress}%` : `${j.durationS}s` }),
    el("dt", { text: "Task" }), el("dd", { text: j.taskId || "—" }),
  );

  const model = j.model?.exists ? j.model.path : null;
  const preview = j.preview?.exists ? { path: j.preview.path, caption: modelJobLabel(j), model } : null;
  const parts = [
    el("div", { class: "detail-prompt", text: modelJobLabel(j) }),
    j.error ? el("div", { class: "detail-error", text: j.error }) : null,
    facts,
    preview ? el("h3", { text: "Model preview" }) : null,
    preview ? el("div", { class: "detail-gallery" }, resultCard(preview, () => [preview], "3D")) : null,
    el("h3", { text: `Sent to Tripo (${inputs.length})` }),
    el("div", { class: "detail-gallery" }, ...inputs.map(tile)),
    el(
      "div",
      { class: "detail-actions" },
      model ? el("button", { class: "button primary", text: "Open 3D model", onclick: () => imageAction("openModel", null, model) }) : null,
      model ? el("button", { class: "button", text: "Rig & animate…", onclick: () => openRigDialog(j.id) }) : null,
      el("button", { class: "button", text: "Show in folder", onclick: () => (model ? invoke("show_in_folder", { path: model }) : invoke("open_path", { path: j.folder })) }),
      j.source.exists ? el("button", { class: "button", text: "Make another…", onclick: () => openModelDialog(j.source.path) }) : null,
    ),
  ];
  $("history-detail").replaceChildren(...parts.filter(Boolean));
}

function renderHistoryDetail() {
  const box = $("history-detail");
  const job = state.modelJobs.find((x) => x.id === state.selectedId);
  if (job) {
    renderModelJobDetail(job);
    return;
  }
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

// ---- reference sheet preset ---------------------------------------------------------------------------------------
// Builds a turnaround-sheet prompt: one image with the same subject from several angles, lit flat on a plain
// background, which is what image-to-3D tools want as input.

const SHEET_STORAGE_KEY = "referenceSheet";
const SHEET_VIEWS = {
  front: "front view",
  threeQuarter: "three-quarter front view",
  side: "side view (profile)",
  back: "back view",
  top: "top-down view",
};
// tile: rough width/height of one view, used to pick the aspect ratio.
const SHEET_KINDS = {
  character: { noun: "character", whole: "the whole body visible from head to feet", tile: 0.55, views: ["front", "threeQuarter", "side", "back"] },
  creature: { noun: "creature", whole: "the whole body visible, including any tail, wings or horns", tile: 0.8, views: ["front", "threeQuarter", "side", "back"] },
  item: { noun: "item", whole: "the whole object visible", tile: 0.9, views: ["front", "side", "back", "top"] },
};
const SHEET_POSES = {
  character: {
    a: "neutral A-pose: standing straight, arms angled down and away from the body at about 45 degrees, legs slightly apart, hands open, neutral expression",
    t: "neutral T-pose: standing straight, arms held straight out to the sides at shoulder height, legs slightly apart, hands open, neutral expression",
    relaxed: "relaxed neutral standing pose, arms at the sides without touching the body, neutral expression",
  },
  creature: {
    a: "neutral standing pose, legs slightly apart, and any arms or wings held slightly away from the body so no limb hides the torso",
    t: "neutral standing pose with any arms or wings spread fully out to the sides",
    relaxed: "relaxed natural stance",
  },
};

function sheetValues() {
  return {
    kind: $("sheet-kind").value,
    subject: $("sheet-subject").value.trim(),
    useRefs: $("sheet-use-refs").checked,
    style: $("sheet-style").value.trim(),
    views: [...$("sheet-views").querySelectorAll("input:checked")].map((c) => c.value),
    pose: $("sheet-pose").value,
    background: $("sheet-background").value,
    for3d: $("sheet-for-3d").checked,
    setAspect: $("sheet-set-aspect").checked,
  };
}

function setSheetViews(views) {
  $("sheet-views").querySelectorAll("input").forEach((c) => (c.checked = views.includes(c.value)));
}

function refsPhrase() {
  const n = state.references.length;
  return n === 1 ? "image 1" : `images 1–${n}`;
}

const sheetUsesRefs = (v) => v.useRefs && state.references.length > 0;

// The supported aspect ratio closest to the views laid out side by side.
function sheetAspect(v) {
  const target = Math.max(1, v.views.length) * SHEET_KINDS[v.kind].tile;
  const supported = state.info?.aspectRatios ?? [];
  let best = null;
  for (const a of ["1:2", "2:3", "3:4", "1:1", "4:3", "3:2", "16:9", "2:1", "21:9", "5:2"]) {
    if (!supported.includes(a)) continue;
    const [w, h] = a.split(":").map(Number);
    const d = Math.abs(Math.log(w / h / target));
    if (!best || d < best.d) best = { a, d };
  }
  return best?.a ?? null;
}

function buildSheetPrompt(v) {
  const k = SHEET_KINDS[v.kind];
  const refs = sheetUsesRefs(v);
  const n = v.views.length;
  const lines = [];

  let subject;
  if (v.subject && refs) subject = `${v.subject}, based on the ${k.noun} in ${refsPhrase()}`;
  else if (v.subject) subject = v.subject;
  else subject = `the ${k.noun} shown in ${refsPhrase()}`;
  lines.push(`${k.noun[0].toUpperCase()}${k.noun.slice(1)} reference sheet (model turnaround) of ${subject}.`);
  if (refs) {
    lines.push(
      v.kind === "item"
        ? "Keep the design exactly the same as the reference: same shape, proportions, materials, colors and details."
        : "Keep the design exactly the same as the reference: same proportions, face, hair, outfit, colors and details.",
    );
  }

  const viewList = v.views.map((x) => SHEET_VIEWS[x]).join(", ");
  if (n === 1) {
    lines.push(`Show a single ${viewList}, centered, with ${k.whole}.`);
  } else {
    lines.push(
      `Show ${n} views of the same ${k.noun} side by side in one row, left to right: ${viewList}. ` +
        `Every view is the same size and scale, ${v.kind === "item" ? "centered on the same line" : "standing on the same ground line"}, ` +
        `evenly spaced and not overlapping, with ${k.whole}.`,
    );
  }

  if (v.kind !== "item") lines.push(`Pose: ${SHEET_POSES[v.kind][v.pose]}. The pose is identical in every view.`);
  if (v.style) lines.push(`Style: ${v.style}.`);
  if (v.for3d) {
    lines.push(
      "Orthographic views with no perspective distortion. Flat, even, diffuse lighting with no cast shadows, " +
        "no strong highlights and no rim light, so the colors show the true surface.",
    );
  }
  lines.push(`Plain solid ${v.background} background. No text, labels, arrows, grid lines, color swatches, ground plane or extra objects.`);
  return lines.join("\n");
}

// Why the sheet can't be built yet, or null when it can.
function sheetProblem(v) {
  if (!v.views.length) return "Pick at least one view.";
  if (!v.subject && !sheetUsesRefs(v)) return "Describe the subject, or add a reference image of it.";
  return null;
}

function renderSheet() {
  const v = sheetValues();
  const hasRefs = state.references.length > 0;
  $("sheet-use-refs").disabled = !hasRefs;
  $("sheet-use-refs-text").textContent = hasRefs
    ? `Base the design on ${refsPhrase()}`
    : "Base the design on reference images (none added)";
  $("sheet-pose").disabled = v.kind === "item";
  const aspect = sheetAspect(v);
  $("sheet-set-aspect").disabled = !aspect;
  $("sheet-aspect-text").textContent = aspect ? `Set aspect ratio to ${aspect}` : "Set aspect ratio";
  const problem = sheetProblem(v);
  $("sheet-apply").disabled = problem !== null;
  $("sheet-preview").value = problem ?? buildSheetPrompt(v);
}

function openSheet() {
  let saved = null;
  try {
    saved = JSON.parse(localStorage.getItem(SHEET_STORAGE_KEY) || "null");
  } catch {
    saved = null;
  }
  if (saved && SHEET_KINDS[saved.kind]) {
    $("sheet-kind").value = saved.kind;
    $("sheet-subject").value = saved.subject ?? "";
    $("sheet-use-refs").checked = saved.useRefs ?? true;
    $("sheet-style").value = saved.style ?? "";
    setSheetViews(Array.isArray(saved.views) && saved.views.length ? saved.views : SHEET_KINDS[saved.kind].views);
    if (saved.pose in SHEET_POSES.character) $("sheet-pose").value = saved.pose;
    if (saved.background) $("sheet-background").value = saved.background;
    $("sheet-for-3d").checked = saved.for3d ?? true;
    $("sheet-set-aspect").checked = saved.setAspect ?? true;
  } else if (!$("sheet-views").querySelector("input:checked")) {
    setSheetViews(SHEET_KINDS[$("sheet-kind").value].views);
  }
  renderSheet();
  $("sheet-dialog").returnValue = "";
  $("sheet-dialog").showModal();
  $("sheet-subject").focus();
}

function applySheet() {
  const v = sheetValues();
  if (sheetProblem(v)) return;
  try {
    localStorage.setItem(SHEET_STORAGE_KEY, JSON.stringify(v));
  } catch {
    // Only a convenience: the dialog starts from its defaults next time.
  }
  const prompt = buildSheetPrompt(v);
  const current = $("prompt").value.trim();
  if (current && current !== prompt && !confirm("Replace the current prompt with the reference sheet prompt?")) return;
  $("prompt").value = prompt;
  const aspect = sheetAspect(v);
  if (v.setAspect && aspect) $("aspect").value = aspect;
  settingsChanged();
  $("prompt").focus();
}

$("sheet-open").addEventListener("click", openSheet);
$("sheet-kind").addEventListener("change", () => setSheetViews(SHEET_KINDS[$("sheet-kind").value].views));
$("sheet-dialog").addEventListener("input", renderSheet);
$("sheet-dialog").addEventListener("change", renderSheet);
$("sheet-dialog").addEventListener("close", () => {
  if ($("sheet-dialog").returnValue === "apply") applySheet();
});

// ---- 3D models (Tripo) --------------------------------------------------------------------------------------------
// "Make 3D model…" on any image. A reference sheet is cut into separate views (Tripo's multiview input takes one
// image per side); anything else is sent as one picture. Jobs run in the backend, which sends `model-job` events.

const MODEL_SLOTS = [
  ["front", "Front"],
  ["left", "Left"],
  ["back", "Back"],
  ["right", "Right"],
  ["skip", "Don't send"],
];
const modelDialog = { source: null, crops: [], slots: [] };

// Which side each cut-out view probably shows. A sheet made with the preset has its views in the order they were
// picked there; otherwise guess from how many there are.
function defaultSlots(n) {
  let saved = null;
  try {
    saved = JSON.parse(localStorage.getItem(SHEET_STORAGE_KEY) || "null");
  } catch {
    saved = null;
  }
  const fromSheet = { front: "front", threeQuarter: "skip", side: "left", back: "back", top: "skip" };
  if (Array.isArray(saved?.views) && saved.views.length === n) return saved.views.map((v) => fromSheet[v] ?? "skip");
  const guesses = { 2: ["front", "left"], 3: ["front", "left", "back"], 4: ["front", "skip", "left", "back"] };
  return guesses[n] ?? Array.from({ length: n }, (_, i) => ["front", "left", "back", "right"][i] ?? "skip");
}

const modelMode = () => document.querySelector('input[name="model-mode"]:checked')?.value ?? "image";
const modelOptions = () => ({ texture: $("model-texture").value, pbr: $("model-pbr").checked, lowPoly: $("model-lowpoly").checked });

function modelProblem() {
  if (modelMode() !== "views") return null;
  const used = modelDialog.slots.filter((s) => s !== "skip");
  if (!used.includes("front")) return "Set one of the views as Front.";
  if (new Set(used).size !== used.length) return "Each side can only be used once.";
  if (used.length < 2) return "Pick at least two views, or send the whole image instead.";
  return null;
}

function renderModelViews() {
  $("model-views").replaceChildren(
    ...modelDialog.crops.map((c, i) => {
      const select = el(
        "select",
        {
          onchange: (e) => {
            modelDialog.slots[i] = e.target.value;
            renderModelViews();
            renderModelDialog();
          },
        },
        ...MODEL_SLOTS.map(([value, label]) => el("option", { value, text: label })),
      );
      select.value = modelDialog.slots[i];
      return el(
        "div",
        { class: `model-view${modelDialog.slots[i] === "skip" ? " skipped" : ""}` },
        el("img", { src: imgSrc(c.path), alt: `View ${i + 1}`, draggable: "false" }),
        select,
      );
    }),
  );
}

async function renderModelDialog() {
  $("model-views-box").hidden = modelMode() !== "views";
  $("model-pbr").disabled = $("model-texture").value === "none";
  const problem = modelProblem();
  $("model-problem").hidden = !problem;
  $("model-problem").textContent = problem ?? "";
  $("model-start").disabled = problem !== null;
  try {
    const est = await invoke("estimate_model_cost", { options: modelOptions() });
    $("model-estimate").textContent = `${money(est.usd)} (${est.credits} credits)`;
  } catch (e) {
    $("model-estimate").textContent = "—";
  }
}

async function openModelDialog(path) {
  if (state.info?.tripoKeySource === "none") {
    openSettings();
    toast("Add your Tripo API key first.", "error");
    $("tripo-key-input").focus();
    return;
  }
  modelDialog.source = path;
  try {
    modelDialog.crops = await invoke("split_sheet", { path });
  } catch {
    modelDialog.crops = [];
  }
  modelDialog.slots = defaultSlots(modelDialog.crops.length);
  const sheet = modelDialog.crops.length >= 2;
  const viewsRadio = document.querySelector('input[name="model-mode"][value="views"]');
  viewsRadio.disabled = !sheet;
  viewsRadio.checked = sheet;
  document.querySelector('input[name="model-mode"][value="image"]').checked = !sheet;
  $("model-mode-hint").textContent = sheet
    ? `Found ${modelDialog.crops.length} separate views. Check which side each one shows.`
    : "No separate views were found, so the image goes as one picture.";
  $("model-source-img").src = imgSrc(path);
  renderModelViews();
  renderModelDialog();

  $("model-balance").textContent = "";
  invoke("tripo_balance")
    .then((b) => ($("model-balance").textContent = `· ${Math.floor(b).toLocaleString()} credits left`))
    .catch(() => ($("model-balance").textContent = "· couldn't check your balance"));
  $("model-dialog").returnValue = "";
  $("model-dialog").showModal();
}

async function startModel() {
  if (modelProblem()) return;
  const inputs =
    modelMode() === "views"
      ? modelDialog.crops
          .map((c, i) => ({ view: modelDialog.slots[i], path: c.path }))
          .filter((x) => x.view !== "skip")
      : [{ view: "image", path: modelDialog.source }];
  try {
    const job = await invoke("make_3d_model", { source: modelDialog.source, inputs, options: modelOptions() });
    upsertModelJob(job);
    showModelCard(job);
    toast("Sent to Tripo. The model usually takes a minute or two.", "ok");
  } catch (e) {
    toast(`Couldn't start the 3D model: ${e}`, "error");
  }
  loadHistory();
}

function upsertModelJob(job) {
  const i = state.modelJobs.findIndex((x) => x.id === job.id);
  const previous = i >= 0 ? state.modelJobs[i] : null;
  if (i >= 0) state.modelJobs[i] = job;
  else state.modelJobs.unshift(job);
  return previous;
}

const JOB_WORDS = {
  model: { running: "Making 3D model…", done: "3D model", badge: "3D" },
  rig: { running: "Rigging…", done: "Rigged model", badge: "RIG" },
  animate: { running: "Animating…", done: "Animated model", badge: "ANIM" },
};

// The Results card for a job: progress while it runs, then the model's preview render (rig and animate jobs show
// the preview of the model they were made from).
function modelJobCard(j) {
  const words = JOB_WORDS[j.kind] ?? JOB_WORDS.model;
  if (j.status === "running") {
    return el(
      "div",
      { class: "card pending", "data-job": j.id, title: modelJobLabel(j) },
      el(
        "div",
        {},
        el("div", { class: "spinner" }),
        el("p", { text: `${words.running} ${j.progress}%` }),
        el("div", { class: "progress" }, el("div", { style: `width: ${j.progress}%` })),
      ),
    );
  }
  if (j.status !== "ok" || !j.model) return null;
  const caption = `${modelJobLabel(j)} · ${money(j.costUsd)}`;
  const preview = j.kind === "model" ? j.preview : rootModelJob(j)?.preview;
  if (!preview?.exists) {
    return el("div", {
      class: "card model-placeholder",
      "data-job": j.id,
      title: caption,
      text: `${words.done} ready. Right-click to open it.`,
      oncontextmenu: (e) => showMenu(e, j.model.path, j.model.path),
    });
  }
  const entry = { path: preview.path, caption, model: j.model.path };
  const card = resultCard(entry, () => (j.kind === "model" ? state.results : [entry]), words.badge);
  card.dataset.job = j.id;
  return card;
}

function showModelCard(j) {
  const card = modelJobCard(j);
  const existing = $("results").querySelector(`[data-job="${j.id}"]`);
  if (j.kind === "model" && j.status === "ok" && j.preview?.exists && !state.results.some((r) => r.path === j.preview.path)) {
    state.results.unshift({ path: j.preview.path, caption: `${modelJobLabel(j)} · ${money(j.costUsd)}`, model: j.model.path });
  }
  if (existing && card) existing.replaceWith(card);
  else if (existing) existing.remove();
  else if (card) {
    $("results").insertBefore(card, $("results-empty").nextSibling);
    $("results-empty").hidden = true;
  }
}

listen("model-job", (e) => {
  const j = e.payload;
  const previous = upsertModelJob(j);
  showModelCard(j);
  if (j.status !== "running" && (!previous || previous.status === "running")) {
    const done = (JOB_WORDS[j.kind] ?? JOB_WORDS.model).done;
    if (j.status === "ok") {
      const next = j.kind === "rig" && j.options?.animations?.length ? " Animations are next." : "";
      toast(`${done} ready for ${money(j.costUsd)}. Right-click its card to open it.${next}`, "ok");
    } else {
      toast(`${done} failed: ${j.error}`, "error");
    }
    refreshCosts();
  }
  if ($("tab-history").classList.contains("active")) renderHistory();
});

document.querySelectorAll('input[name="model-mode"]').forEach((r) => r.addEventListener("change", renderModelDialog));
["model-texture", "model-pbr", "model-lowpoly"].forEach((id) => $(id).addEventListener("change", renderModelDialog));
$("model-dialog").addEventListener("close", () => {
  if ($("model-dialog").returnValue === "start") startModel();
});

// ---- rig & animate (Tripo) ----------------------------------------------------------------------------------------
// Rigging builds on a finished model job. Tripo's free rig check suggests the body type; animations (up to 5) are
// applied by a second job once the rig is done.

const rigDialog = { jobId: null, picked: new Set() };

const rigAnimationsFor = (type) => (state.info?.rigAnimations ?? []).find(([t]) => t === type)?.[1] ?? [];

function rigOptions() {
  const rigType = $("rig-type").value;
  const available = rigAnimationsFor(rigType);
  return {
    rigType,
    spec: $("rig-spec").value,
    format: $("rig-format").value,
    animations: available.filter((a) => rigDialog.picked.has(a)),
    inPlace: $("rig-in-place").checked,
  };
}

// The usual picks when the body type changes: idle and walk for humanoids, the one move cycle for the others.
function defaultAnimations(type) {
  const available = rigAnimationsFor(type);
  const wanted = type === "biped" ? ["preset:idle", "preset:walk"] : available;
  rigDialog.picked = new Set(wanted.filter((a) => available.includes(a)));
}

async function renderRig() {
  const o = rigOptions();
  const max = state.info?.maxAnimations ?? 5;
  $("rig-spec").disabled = o.rigType !== "biped";
  const available = rigAnimationsFor(o.rigType);
  if (!available.length) {
    $("rig-animations").replaceChildren(el("span", { class: "muted small", text: "Tripo has no preset animations for this body type yet. You get the rigged model." }));
  } else {
    $("rig-animations").replaceChildren(
      ...available.map((a) => {
        const box = el("input", {
          type: "checkbox",
          onchange: (e) => {
            if (e.target.checked) rigDialog.picked.add(a);
            else rigDialog.picked.delete(a);
            renderRig();
          },
        });
        box.checked = rigDialog.picked.has(a);
        box.disabled = !box.checked && o.animations.length >= max;
        return el("label", { class: "check" }, box, ` ${animationName(a)}`);
      }),
    );
  }
  $("rig-in-place").disabled = !o.animations.length;
  $("rig-start").textContent = o.animations.length ? "Rig and animate" : "Rig model";
  const tooMany = o.animations.length > max;
  $("rig-problem").hidden = !tooMany;
  $("rig-problem").textContent = tooMany ? `Pick at most ${max} animations.` : "";
  $("rig-start").disabled = tooMany;
  try {
    const est = await invoke("estimate_rig_cost", { options: o });
    $("rig-estimate").textContent = money(est.usd);
    $("rig-estimate-detail").textContent =
      `rig ${est.rigCredits} credits` + (o.animations.length ? ` + ${plural(o.animations.length, "animation")} ${est.animationCredits} credits` : "");
  } catch {
    $("rig-estimate").textContent = "—";
    $("rig-estimate-detail").textContent = "";
  }
}

async function openRigDialog(jobId) {
  if (state.info?.tripoKeySource === "none") {
    openSettings();
    toast("Add your Tripo API key first.", "error");
    return;
  }
  rigDialog.jobId = jobId;
  const check = $("rig-check");
  check.className = "rig-check small muted";
  check.textContent = "Asking Tripo what kind of body this is (free)…";
  defaultAnimations($("rig-type").value);
  renderRig();
  $("rig-dialog").returnValue = "";
  $("rig-dialog").showModal();
  try {
    const r = await invoke("check_rig", { jobId });
    if (rigDialog.jobId !== jobId) return;
    if (r.rigType && [...$("rig-type").options].some((opt) => opt.value === r.rigType)) {
      $("rig-type").value = r.rigType;
      defaultAnimations(r.rigType);
    }
    check.className = r.riggable ? "rig-check small" : "rig-check small warn";
    const body = RIG_TYPE_NAMES[r.rigType] ?? r.rigType ?? "";
    check.textContent = r.riggable
      ? `Tripo sees ${/^[aeiou]/.test(body) ? "an" : "a"} ${body} body and can rig it. Change the body type if that's wrong.`
      : "Tripo doesn't think this model can be rigged. You can still try with the body type you pick, but it may fail.";
  } catch (e) {
    if (rigDialog.jobId !== jobId) return;
    check.className = "rig-check small warn";
    check.textContent = `Couldn't check the model: ${e}. Pick the body type yourself.`;
  }
  renderRig();
}

async function startRig() {
  try {
    const job = await invoke("rig_model", { jobId: rigDialog.jobId, options: rigOptions() });
    upsertModelJob(job);
    showModelCard(job);
    showTab("generate");
    toast("Sent to Tripo for rigging. It usually takes a minute or two.", "ok");
  } catch (e) {
    toast(`Couldn't start rigging: ${e}`, "error");
  }
  loadHistory();
}

$("rig-type").addEventListener("change", () => {
  defaultAnimations($("rig-type").value);
  renderRig();
});
["rig-spec", "rig-format", "rig-in-place"].forEach((id) => $(id).addEventListener("change", renderRig));
$("rig-dialog").addEventListener("close", () => {
  if ($("rig-dialog").returnValue === "start") startRig();
});

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
  document.querySelector('.lightbox-bar [data-action="openModel"]').hidden = !item.model;
  document.querySelector('.lightbox-bar [data-action="model"]').hidden = !!item.model;
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
const lightboxItem = () => state.lightbox.list[state.lightbox.index];
$("lightbox-img").addEventListener("contextmenu", (e) => showMenu(e, lightboxItem().path, lightboxItem().model));
document.querySelectorAll(".lightbox-bar [data-action]").forEach((b) =>
  b.addEventListener("click", () => imageAction(b.dataset.action, lightboxItem().path, lightboxItem().model)),
);

// `model` is set for a 3D model's preview render: the menu then offers the model instead of making one.
function showMenu(e, path, model = null) {
  e.preventDefault();
  e.stopPropagation();
  state.menuPath = path;
  state.menuModel = model;
  const menu = $("context-menu");
  menu.querySelector('[data-action="openModel"]').hidden = !model;
  menu.querySelector('[data-action="model"]').hidden = !!model;
  menu.querySelector('[data-action="rig"]').hidden = !riggableJobFor(model);
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
  b.addEventListener("click", () => imageAction(b.dataset.action, state.menuPath, state.menuModel)),
);

async function imageAction(action, path, model = null) {
  hideMenu();
  try {
    if (action === "open") await invoke("open_path", { path });
    if (action === "folder") await invoke("show_in_folder", { path: model || path });
    if (action === "openModel") await invoke("open_path", { path: model });
    if (action === "rig" && riggableJobFor(model)) {
      closeLightbox();
      await openRigDialog(riggableJobFor(model).id);
    }
    if (action === "model") {
      closeLightbox();
      await openModelDialog(path);
    }
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

// ---- audio --------------------------------------------------------------------------------------------------------
// Sound effects and music from ElevenLabs (cloud), the app's own audio server or ComfyUI (self-hosted on this PC,
// another PC or a cloud GPU such as Lambda). The backend lists every usable source; the controls adapt to it.

const audioState = { sources: [], servers: [], workflows: [], jobs: [], pending: [], elevenKey: false, usage: null, usageAt: 0 };
const ELEVEN_MODEL_NAMES = {
  eleven_text_to_sound_v2: "Sound effects v2",
  eleven_text_to_sound_v3: "Sound effects v3",
  music_v1: "Music v1",
  music_v2: "Music v2",
  music_v2_5: "Music v2.5",
};
const KIND_NAMES = { sfx: "sound effect", music: "music" };

const audioPrefs = () => state.settings?.audio || {};
const saveAudioPrefs = debounce(async () => {
  if (!state.settings) return;
  try {
    await invoke("save_settings", { settings: state.settings });
  } catch (e) {
    console.error(e);
  }
}, 600);
// Last-used Audio tab choices live in settings.json with the rest.
function rememberAudio(changes) {
  if (!state.settings) return;
  state.settings = { ...state.settings, audio: { ...audioPrefs(), ...changes } };
  saveAudioPrefs();
}

const currentAudioSource = () => audioState.sources.find((s) => s.id === $("audio-source").value);

async function loadAudioSources() {
  try {
    const r = await invoke("list_audio_sources");
    audioState.sources = r.sources;
    audioState.servers = r.servers;
    audioState.elevenKey = r.elevenlabsKey;
  } catch (e) {
    toast(`Couldn't list the audio sources: ${e}`, "error");
  }
  try {
    audioState.workflows = await invoke("list_comfy_workflows");
  } catch {
    audioState.workflows = [];
  }
  renderAudioSources();
  renderServerSettings();
}

function renderAudioSources() {
  const sel = $("audio-source");
  const keep = sel.value || audioPrefs().source;
  const groups = new Map();
  for (const s of audioState.sources) {
    if (!groups.has(s.group)) groups.set(s.group, []);
    groups.get(s.group).push(s);
  }
  const nodes = [...groups].map(([group, list]) =>
    el("optgroup", { label: group }, ...list.map((s) => el("option", { value: s.id, text: `${s.label} (${KIND_NAMES[s.kind]})` }))),
  );
  // Servers that are down, or ComfyUI servers without workflows, still show so it's clear why.
  for (const srv of audioState.servers) {
    if (srv.ok && !(srv.kind === "comfyui" && !audioState.workflows.length)) continue;
    const why = srv.ok ? "import a workflow in Servers…" : "can't connect";
    nodes.push(el("optgroup", { label: srv.name }, el("option", { value: "", disabled: true, text: `(${why})` })));
  }
  sel.replaceChildren(...nodes);
  if (audioState.sources.some((s) => s.id === keep)) sel.value = keep;
  audioSourceChanged(false);
}

function showRow(labelId, controlId, visible) {
  $(labelId).hidden = !visible;
  $(controlId).hidden = !visible;
}

function audioSourceChanged(remember = true) {
  const s = currentAudioSource();
  $("audio-generate").disabled = !s;
  if (!s) {
    $("audio-source-info").textContent = "No audio source is available.";
    return;
  }
  const prefs = audioPrefs();
  showRow("audio-model-label", "audio-model", s.models.length > 0);
  showRow("audio-influence-label", "audio-influence-wrap", s.influence);
  showRow("audio-negative-label", "audio-negative", s.negative);
  showRow("audio-seed-label", "audio-seed", s.seed);
  $("audio-loop-wrap").hidden = !s.looping;
  $("audio-instrumental-wrap").hidden = !s.instrumental;
  $("audio-auto-wrap").hidden = !s.autoLength;

  if (s.models.length) {
    const keepModel = prefs.models?.[s.id];
    $("audio-model").replaceChildren(...s.models.map((m) => el("option", { value: m, text: ELEVEN_MODEL_NAMES[m] ?? m })));
    if (s.models.includes(keepModel)) $("audio-model").value = keepModel;
  }
  const secs = $("audio-seconds");
  secs.min = s.minSeconds;
  secs.max = s.maxSeconds;
  const wanted = prefs.seconds?.[s.kind] ?? (s.kind === "music" ? 60 : 3);
  secs.value = Math.min(s.maxSeconds, Math.max(s.minSeconds, wanted));
  $("audio-auto").checked = s.autoLength && (prefs.auto?.[s.id] ?? false);
  secs.disabled = $("audio-auto").checked;
  $("audio-prompt").placeholder =
    s.kind === "music"
      ? 'Describe the music, e.g. "dark ambient dungeon loop, low strings and distant choir, slow, 70 BPM"'
      : 'Describe the sound, e.g. "heavy sword hitting a wooden shield, close, dry"';

  const info = $("audio-source-info");
  info.classList.remove("warn");
  if (s.provider === "elevenlabs" && !audioState.elevenKey) {
    info.textContent = "Add your ElevenLabs API key in Settings to use this.";
    info.classList.add("warn");
  } else {
    const bits = [
      s.provider === "elevenlabs" ? "Cloud" : s.provider === "comfyui" ? "ComfyUI workflow" : "Self-hosted model",
      `up to ${s.maxSeconds >= 60 ? `${Math.round(s.maxSeconds / 6) / 10} min` : `${s.maxSeconds} s`}`,
      s.device ? `runs on ${s.device.toUpperCase()}` : null,
      s.license,
    ];
    info.textContent = bits.filter(Boolean).join(" · ");
  }
  renderAudioUsage();
  if (remember) rememberAudio({ source: s.id });
}

async function renderAudioUsage() {
  const s = currentAudioSource();
  if (s?.provider !== "elevenlabs" || !audioState.elevenKey) {
    $("audio-usage").textContent = s && s.provider !== "elevenlabs" ? "Self-hosted: no per-sound cost." : "";
    return;
  }
  if (!audioState.usage || Date.now() - audioState.usageAt > 60000) {
    try {
      audioState.usage = await invoke("elevenlabs_usage");
      audioState.usageAt = Date.now();
    } catch {
      $("audio-usage").textContent = "";
      return;
    }
  }
  const u = audioState.usage;
  $("audio-usage").textContent = `ElevenLabs ${u.tier}: ${Math.round(u.used).toLocaleString()} of ${Math.round(u.limit).toLocaleString()} credits used this period`;
}

function audioRequest(prompt) {
  const s = currentAudioSource();
  const seed = $("audio-seed").value.trim();
  return {
    source: s.id,
    prompt,
    negative: s.negative ? $("audio-negative").value.trim() || null : null,
    seconds: s.autoLength && $("audio-auto").checked ? null : Number($("audio-seconds").value) || null,
    variations: Math.min(4, Math.max(1, parseInt($("audio-variations").value, 10) || 1)),
    seed: s.seed && seed !== "" ? Number(seed) : null,
    looping: s.looping && $("audio-loop").checked,
    influence: s.influence ? Number($("audio-influence").value) : null,
    instrumental: s.instrumental && $("audio-instrumental").checked,
    model: s.models.length ? $("audio-model").value : null,
    clean: $("audio-clean").checked,
  };
}

async function generateAudio() {
  const s = currentAudioSource();
  if (!s) {
    toast("Pick what to generate with first.", "error");
    return;
  }
  if (s.provider === "elevenlabs" && !audioState.elevenKey) {
    openSettings();
    toast("Add your ElevenLabs API key first.", "error");
    $("eleven-key-input").focus();
    return;
  }
  const text = $("audio-prompt").value;
  const prompts = $("audio-batch").checked
    ? text.split("\n").map((l) => l.trim()).filter(Boolean)
    : [text.trim()].filter(Boolean);
  if (!prompts.length) {
    toast("Describe the sound first.", "error");
    $("audio-prompt").focus();
    return;
  }
  const requests = prompts.map(audioRequest);
  for (const request of requests) {
    const pending = { id: `pending-${Math.random()}`, request, label: `${s.group} · ${s.label}` };
    audioState.pending.unshift(pending);
    renderAudioList();
    try {
      const job = await invoke("generate_audio", { request });
      audioState.jobs.unshift(job);
      const credits = job.credits != null ? ` (${Math.round(job.credits).toLocaleString()} credits)` : "";
      toast(`Made ${plural(job.clips.length, "clip")} for "${job.prompt.slice(0, 40)}"${credits}.`, "ok", 4000);
    } catch (e) {
      toast(`Audio failed: ${e}`, "error");
    } finally {
      audioState.pending = audioState.pending.filter((p) => p !== pending);
      audioState.usage = null;
      renderAudioList();
      renderAudioUsage();
      refreshCosts();
    }
  }
  await loadAudioJobs();
}

async function loadAudioJobs() {
  try {
    audioState.jobs = await invoke("list_audio_jobs");
  } catch (e) {
    toast(`Couldn't load the audio log: ${e}`, "error");
  }
  renderAudioList();
}

function audioCostText(j) {
  if (j.provider !== "elevenlabs") return "self-hosted";
  if (j.credits == null) return j.status === "ok" ? "credits unknown" : "not charged";
  return `${Math.round(j.credits).toLocaleString()} credits · ${money(j.costUsd)}`;
}

function audioJobCard(j) {
  const model = ELEVEN_MODEL_NAMES[j.model] ?? j.model;
  const meta = [j.sourceName, model, KIND_NAMES[j.kind], formatWhen(j.createdAt), audioCostText(j)].filter(Boolean).join(" · ");
  const clips = j.clips.map((c) =>
    el(
      "div",
      { class: "audio-clip", title: c.path },
      c.exists ? el("audio", { controls: true, preload: "none", src: imgSrc(c.path) }) : el("span", { class: "muted small", text: "File missing" }),
      el("span", { class: "muted small", text: c.seconds != null ? `${c.seconds.toFixed(1)} s` : "" }),
      c.exists ? el("button", { class: "button small", text: "Show", title: "Show in folder", onclick: () => invoke("show_in_folder", { path: c.path }) }) : null,
    ),
  );
  return el(
    "div",
    { class: `audio-job${j.status === "ok" ? "" : " failed"}` },
    el(
      "div",
      { class: "audio-job-head" },
      el("span", { class: "audio-job-prompt", text: j.prompt }),
      el("span", { class: "muted small audio-job-meta", text: meta }),
    ),
    j.error ? el("div", { class: "detail-error small", text: j.error }) : null,
    clips.length ? el("div", { class: "audio-clips" }, ...clips) : null,
    el(
      "div",
      { class: "audio-job-actions" },
      el("button", { class: "button small", text: "Use prompt", onclick: () => reuseAudio(j) }),
    ),
  );
}

function reuseAudio(j) {
  $("audio-batch").checked = false;
  $("audio-prompt").value = j.prompt;
  const src = j.params?.source;
  if (src && audioState.sources.some((s) => s.id === src)) {
    $("audio-source").value = src;
    audioSourceChanged();
  }
  if (j.params?.seconds) $("audio-seconds").value = j.params.seconds;
  if (j.params?.variations) $("audio-variations").value = j.params.variations;
  if (j.params?.negative) $("audio-negative").value = j.params.negative;
  $("audio-prompt").focus();
}

function renderAudioList() {
  const q = $("audio-search").value.trim().toLowerCase();
  const jobs = audioState.jobs.filter((j) => !q || j.prompt.toLowerCase().includes(q));
  const pending = audioState.pending.map((p) =>
    el(
      "div",
      { class: "audio-job" },
      el(
        "div",
        { class: "audio-job-head" },
        el("div", { class: "spinner" }),
        el("span", { class: "audio-job-prompt", text: p.request.prompt }),
        el("span", { class: "muted small audio-job-meta", text: `${p.label} · making ${plural(p.request.variations, "variation")}…` }),
      ),
    ),
  );
  const shown = jobs.slice(0, 150).map(audioJobCard);
  $("audio-empty").hidden = pending.length + shown.length > 0;
  $("audio-list").replaceChildren($("audio-empty"), ...pending, ...shown);
  const clips = audioState.jobs.reduce((n, j) => n + j.clips.length, 0);
  $("audio-summary").textContent = audioState.jobs.length ? `${plural(clips, "clip")} from ${plural(audioState.jobs.length, "prompt")}` : "";
}

$("audio-source").addEventListener("change", () => audioSourceChanged());
$("audio-generate").addEventListener("click", generateAudio);
$("audio-search").addEventListener("input", renderAudioList);
$("audio-servers-open").addEventListener("click", () => {
  openSettings();
  $("server-name").scrollIntoView({ block: "center" });
});
$("audio-auto").addEventListener("change", () => {
  $("audio-seconds").disabled = $("audio-auto").checked;
  const s = currentAudioSource();
  if (s) rememberAudio({ auto: { ...(audioPrefs().auto || {}), [s.id]: $("audio-auto").checked } });
});
$("audio-seconds").addEventListener("change", () => {
  const s = currentAudioSource();
  if (s) rememberAudio({ seconds: { ...(audioPrefs().seconds || {}), [s.kind]: Number($("audio-seconds").value) } });
});
$("audio-model").addEventListener("change", () => {
  const s = currentAudioSource();
  if (s) rememberAudio({ models: { ...(audioPrefs().models || {}), [s.id]: $("audio-model").value } });
});
$("audio-influence").addEventListener("input", () => ($("audio-influence-value").textContent = Number($("audio-influence").value).toFixed(2)));
["audio-variations", "audio-clean", "audio-loop", "audio-instrumental"].forEach((id) =>
  $(id).addEventListener("change", () =>
    rememberAudio({
      variations: Number($("audio-variations").value),
      clean: $("audio-clean").checked,
      loop: $("audio-loop").checked,
      instrumental: $("audio-instrumental").checked,
    }),
  ),
);

function restoreAudioPrefs() {
  const p = audioPrefs();
  if (p.variations) $("audio-variations").value = p.variations;
  if (p.clean != null) $("audio-clean").checked = p.clean;
  if (p.loop != null) $("audio-loop").checked = p.loop;
  if (p.instrumental != null) $("audio-instrumental").checked = p.instrumental;
}

// ---- audio servers and ComfyUI workflows (in Settings) ----

let editingServer = null;

function renderServerSettings() {
  const kindName = { comfyui: "ComfyUI", audioServer: "Audio server" };
  const known = new Map(audioState.servers.map((s) => [s.id, s]));
  const servers = state.settings?.audioServers ?? [];
  $("server-list").replaceChildren(
    ...(audioState.servers.length ? audioState.servers : servers).map((s) => {
      const st = known.get(s.id);
      return el(
        "div",
        { class: "server-row" },
        el("span", { class: `server-dot ${st ? (st.ok ? "ok" : "down") : ""}`, title: st ? (st.ok ? "Connected" : "Can't connect") : "" }),
        el(
          "div",
          { class: "server-text" },
          el("span", { text: `${s.name} · ${kindName[s.kind] ?? s.kind} · ${s.url}${st?.hasToken ? " · token" : ""}` }),
          st ? el("span", { class: "muted small", text: st.detail, title: st.detail }) : null,
        ),
        el("button", { type: "button", class: "button small", text: "Edit", onclick: () => editServer(s) }),
        el("button", { type: "button", class: "button small", text: "Remove", onclick: () => removeServer(s) }),
      );
    }),
  );
  if (!$("server-list").children.length) $("server-list").append(el("p", { class: "muted small", text: "No servers yet." }));

  $("workflow-list").replaceChildren(
    ...audioState.workflows.map((w) =>
      el(
        "div",
        { class: "server-row" },
        el(
          "div",
          { class: "server-text" },
          el("span", { text: `${w.name} · ${KIND_NAMES[w.kind]} · up to ${w.maxSeconds} s` }),
          el("span", { class: "muted small", text: `Fills: ${["prompt", "negative", "seconds", "seed"].filter((r) => w.mapping[r]?.length).join(", ")}` }),
        ),
        el("button", { type: "button", class: "button small", text: "Remove", onclick: () => removeWorkflow(w) }),
      ),
    ),
  );
  if (!audioState.workflows.length) $("workflow-list").append(el("p", { class: "muted small", text: "No workflows yet." }));
}

function editServer(s) {
  editingServer = s;
  $("server-name").value = s.name;
  $("server-kind").value = s.kind;
  $("server-url").value = s.url;
  $("server-token").value = "";
  $("server-token").placeholder = "Token (leave blank to keep)";
  $("server-save").textContent = "Save";
  $("server-cancel").hidden = false;
}

function resetServerForm() {
  editingServer = null;
  ["server-name", "server-url", "server-token"].forEach((id) => ($(id).value = ""));
  $("server-token").placeholder = "Token (optional)";
  $("server-save").textContent = "Add";
  $("server-cancel").hidden = true;
}

async function refreshServerSettings() {
  state.settings = await invoke("get_settings");
  await loadAudioSources();
}

$("server-kind").addEventListener("change", () => {
  $("server-url").placeholder = $("server-kind").value === "comfyui" ? "http://127.0.0.1:8188" : "http://127.0.0.1:8190";
});
$("server-cancel").addEventListener("click", resetServerForm);
$("server-save").addEventListener("click", async () => {
  const token = $("server-token").value.trim();
  const server = {
    id: editingServer?.id ?? "",
    name: $("server-name").value,
    kind: $("server-kind").value,
    url: $("server-url").value || $("server-url").placeholder,
  };
  try {
    // A blank token keeps the saved one when editing; on a new server it means none.
    await invoke("save_audio_server", { server, token: token || (editingServer ? null : "") });
    resetServerForm();
    toast("Server saved. Checking it…", "ok", 2500);
    await refreshServerSettings();
  } catch (e) {
    toast(e, "error");
  }
});

async function removeServer(s) {
  if (!confirm(`Remove ${s.name}? Its sounds stay in the log.`)) return;
  try {
    await invoke("remove_audio_server", { id: s.id });
    await refreshServerSettings();
  } catch (e) {
    toast(e, "error");
  }
}

async function removeWorkflow(w) {
  if (!confirm(`Remove the workflow "${w.name}"?`)) return;
  try {
    await invoke("remove_comfy_workflow", { id: w.id });
    await loadAudioSources();
  } catch (e) {
    toast(e, "error");
  }
}

let workflowDraft = null;

$("workflow-import").addEventListener("click", async () => {
  const path = await openDialog({ title: "ComfyUI workflow (API format)", filters: [{ name: "Workflow", extensions: ["json"] }] });
  if (!path) return;
  try {
    workflowDraft = await invoke("inspect_comfy_workflow", { path });
  } catch (e) {
    toast(e, "error");
    return;
  }
  const d = workflowDraft;
  $("wf-name").value = d.name;
  const describe = (c) => {
    const v = typeof c.value === "string" ? `"${c.value.slice(0, 30)}${c.value.length > 30 ? "…" : ""}"` : String(c.value);
    return `${c.title} › ${c.input} (${v})`;
  };
  document.querySelectorAll("#workflow-dialog select[data-role]").forEach((sel) => {
    const role = sel.dataset.role;
    const detected = d.mapping[role] || [];
    const wantText = role === "prompt" || role === "negative";
    const options = [el("option", { value: "", text: "— not used —" })];
    if (detected.length) {
      const names = detected.map((f) => {
        const c = d.candidates.find((x) => x.node === f.node && x.input === f.input);
        return c ? `${c.title} › ${c.input}` : `${f.node}.${f.input}`;
      });
      options.push(el("option", { value: "auto", text: `Detected: ${names.join(", ")}` }));
    }
    for (const c of d.candidates.filter((c) => (typeof c.value === "string") === wantText)) {
      options.push(el("option", { value: `${c.node}|${c.input}`, text: describe(c) }));
    }
    sel.replaceChildren(...options);
    sel.value = detected.length ? "auto" : "";
  });
  const secs = d.mapping.seconds?.[0];
  const current = secs ? d.candidates.find((c) => c.node === secs.node && c.input === secs.input)?.value : null;
  $("wf-max").value = Math.max(10, Math.ceil(Number(current) || 47));
  $("wf-kind").value = /music|song|ace|minimax|yue/i.test(d.name) ? "music" : "sfx";
  $("wf-problem").hidden = d.mapping.prompt?.length > 0;
  $("wf-problem").textContent = "Pick which input gets the prompt.";
  $("workflow-dialog").returnValue = "";
  $("workflow-dialog").showModal();
});

$("wf-prompt").addEventListener("change", () => ($("wf-problem").hidden = $("wf-prompt").value !== ""));

$("workflow-dialog").addEventListener("close", async () => {
  if ($("workflow-dialog").returnValue !== "save" || !workflowDraft) return;
  const mapping = {};
  document.querySelectorAll("#workflow-dialog select[data-role]").forEach((sel) => {
    const role = sel.dataset.role;
    if (sel.value === "auto") mapping[role] = workflowDraft.mapping[role];
    else if (sel.value === "") mapping[role] = [];
    else {
      const [node, input] = sel.value.split("|");
      mapping[role] = [{ node, input }];
    }
  });
  try {
    const saved = await invoke("save_comfy_workflow", {
      workflow: {
        id: "",
        name: $("wf-name").value,
        kind: $("wf-kind").value,
        maxSeconds: Number($("wf-max").value) || 47,
        mapping,
        workflow: workflowDraft.workflow,
      },
    });
    toast(`Imported "${saved.name}". It's in the Audio tab under each ComfyUI server.`, "ok");
    workflowDraft = null;
    await loadAudioSources();
  } catch (e) {
    toast(e, "error");
  }
});

// ---- settings dialog ----------------------------------------------------------------------------------------------

function renderSettingsDialog() {
  const info = state.info;
  const keyText = (envVar) => ({
    environment: `Using the ${envVar} environment variable. It takes priority over a key saved here.`,
    credentialManager: "A key is saved. Enter a new one to replace it.",
    none: "No key is set.",
  });
  $("key-status").textContent = keyText("XAI_API_KEY")[info.apiKeySource] ?? "";
  $("key-remove").disabled = info.apiKeySource !== "credentialManager";
  $("tripo-key-status").textContent = keyText("TRIPO_API_KEY")[info.tripoKeySource] ?? "";
  $("tripo-key-remove").disabled = info.tripoKeySource !== "credentialManager";
  $("eleven-key-status").textContent = keyText("ELEVENLABS_API_KEY")[info.elevenlabsKeySource] ?? "";
  $("eleven-key-remove").disabled = info.elevenlabsKeySource !== "credentialManager";
  $("eleven-rate").value = state.settings?.elevenlabsUsdPer1kCredits ?? 0.22;
  $("audio-dir").value = info.audioDir;
  $("audio-dir-reset").disabled = !state.settings?.audioDir;
  renderServerSettings();
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

async function setTripoKey(key, message) {
  try {
    await invoke("set_api_key", { key, service: "tripo" });
    $("tripo-key-input").value = "";
    await refreshInfo();
    renderSettingsDialog();
    toast(message, "ok", 3000);
  } catch (e) {
    toast(e, "error");
  }
}
$("tripo-key-save").addEventListener("click", () => {
  const key = $("tripo-key-input").value.trim();
  if (key) setTripoKey(key, "Tripo API key saved.");
});
$("tripo-key-remove").addEventListener("click", () => setTripoKey("", "Saved Tripo API key removed."));

async function setElevenKey(key, message) {
  try {
    await invoke("set_api_key", { key, service: "elevenlabs" });
    $("eleven-key-input").value = "";
    await refreshInfo();
    audioState.usage = null;
    await loadAudioSources();
    renderSettingsDialog();
    toast(message, "ok", 3000);
  } catch (e) {
    toast(e, "error");
  }
}
$("eleven-key-save").addEventListener("click", () => {
  const key = $("eleven-key-input").value.trim();
  if (key) setElevenKey(key, "ElevenLabs API key saved.");
});
$("eleven-key-remove").addEventListener("click", () => setElevenKey("", "Saved ElevenLabs API key removed."));
$("eleven-rate").addEventListener("change", async () => {
  const rate = Math.max(0, Number($("eleven-rate").value) || 0);
  state.settings = { ...state.settings, elevenlabsUsdPer1kCredits: rate };
  try {
    await invoke("save_settings", { settings: state.settings });
  } catch (e) {
    toast(e, "error");
  }
});

async function setAudioDir(dir) {
  try {
    const settings = { ...state.settings, audioDir: dir };
    await invoke("save_settings", { settings });
    state.settings = settings;
    await refreshInfo();
    renderSettingsDialog();
  } catch (e) {
    toast(e, "error");
  }
}
$("audio-dir-change").addEventListener("click", async () => {
  const dir = await openDialog({ directory: true, title: "Folder for new sounds", defaultPath: state.info.audioDir });
  if (dir) setAudioDir(dir);
});
$("audio-dir-reset").addEventListener("click", () => setAudioDir(null));

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
    if (document.querySelector("dialog[open]")) return;
    if ($("tab-generate").classList.contains("active")) generate();
    if ($("tab-audio").classList.contains("active")) generateAudio();
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
  // 3D models still being made when the app last closed: the backend keeps watching them.
  loadHistory().then(() => state.modelJobs.filter((j) => j.status === "running").reverse().forEach(showModelCard));
  restoreAudioPrefs();
  loadAudioSources();
  loadAudioJobs();
  await loadModels();
  $("prompt").focus();
}

init().catch((e) => toast(`Startup failed: ${e}`, "error"));
