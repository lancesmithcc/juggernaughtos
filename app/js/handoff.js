// Browser glue for image hand-offs: pull an image file out of a drop / paste /
// file-picker event, upload it, and copy text to the clipboard.

export function filesFromEvent(ev) {
  return Array.from(
    (ev.dataTransfer && ev.dataTransfer.files) ||
    (ev.clipboardData && ev.clipboardData.files) ||
    (ev.target && ev.target.files) ||
    []);
}

export function imageFromEvent(ev) {
  for (const f of filesFromEvent(ev)) if (f && /^image\//.test(f.type)) return f;
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
  return upload(`/files/handoff/${encodeURIComponent(id)}`, file,
    { "Content-Type": file.type || "application/octet-stream" }, 25);
}

export async function sendChatImages(conversation, text, files) {
  const form = new FormData();
  if (conversation) form.set("conversation_id", conversation);
  form.set("text", text);
  for (const file of files) form.append("image", file);
  return upload("/api/chat/turn", form, { "X-Jugg-Chat": "1" }, 10);
}

async function upload(url, body, headers, maxMb) {
  try {
    const r = await fetch(url, { method: "POST", credentials: "same-origin", headers, body });
    const text = await r.text();
    if (r.ok) return JSON.stringify({ ok: true, json: text });
    let msg = "Upload failed.";
    try {
      msg = JSON.parse(text).error || msg;
    } catch {}
    if (r.status === 413) msg = `Images can be up to ${maxMb} MB each.`;
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
