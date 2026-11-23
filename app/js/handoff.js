// Browser glue for image hand-offs: pull an image file out of a drop / paste /
// file-picker event, upload it, and copy text to the clipboard.

export function imageFromEvent(ev) {
  const list =
    (ev.dataTransfer && ev.dataTransfer.files) ||
    (ev.clipboardData && ev.clipboardData.files) ||
    (ev.target && ev.target.files) ||
    [];
  for (const f of list) if (f && /^image\//.test(f.type)) return f;
  return null;
}

// Dropped straight from a web page, some browsers give only the image's address.
export function droppedUrl(ev) {
  const dt = ev.dataTransfer;
  if (!dt) return "";
  return (dt.getData("text/uri-list") || "").split("\n")[0].trim();
}

// Resolves to {ok:true, json:"…HandoffView…"} or {ok:false, error:"…"}.
export async function uploadHandoff(id, file) {
  try {
    const r = await fetch(`/files/handoff/${encodeURIComponent(id)}`, {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": file.type || "application/octet-stream" },
      body: file,
    });
    const text = await r.text();
    if (r.ok) return JSON.stringify({ ok: true, json: text });
    let msg = "Upload failed.";
    try {
      msg = JSON.parse(text).error || msg;
    } catch {}
    if (r.status === 413) msg = "That file is too big (25 MB at most).";
    return JSON.stringify({ ok: false, error: msg });
  } catch (e) {
    return JSON.stringify({ ok: false, error: "Upload failed: " + (e && e.message ? e.message : e) });
  }
}

export async function copyText(text) {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    return false;
  }
}
