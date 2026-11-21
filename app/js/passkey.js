// WebAuthn ceremonies for Juggernaught. Served same-origin (CSP 'self'),
// imported by the wasm bundle. Returns "ok" or a human-readable message.

const b64u = {
  enc(buf) {
    let s = "";
    for (const b of new Uint8Array(buf)) s += String.fromCharCode(b);
    return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
  },
  dec(str) {
    let s = str.replace(/-/g, "+").replace(/_/g, "/");
    while (s.length % 4) s += "=";
    return Uint8Array.from(atob(s), (c) => c.charCodeAt(0)).buffer;
  },
};

async function post(url, body) {
  const r = await fetch(url, {
    method: "POST",
    credentials: "same-origin",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body ?? {}),
  });
  const j = await r.json().catch(() => ({}));
  if (!r.ok) throw new Error(j.error || `Request failed (${r.status})`);
  return j;
}

function friendly(e) {
  if (e && e.name === "NotAllowedError") return "Cancelled, or the device didn't confirm.";
  if (e && e.name === "InvalidStateError") return "This device already has a passkey here.";
  return (e && e.message) || String(e);
}

export function passkeySupported() {
  return !!(window.PublicKeyCredential && navigator.credentials);
}

export async function passkeyRegister(label) {
  if (!passkeySupported()) return "This browser doesn't support passkeys.";
  try {
    const { state_id, options } = await post("/auth/passkey/register/start");
    const pk = options.publicKey;
    pk.challenge = b64u.dec(pk.challenge);
    pk.user.id = b64u.dec(pk.user.id);
    (pk.excludeCredentials || []).forEach((c) => (c.id = b64u.dec(c.id)));
    const cred = await navigator.credentials.create({ publicKey: pk });
    const res = cred.response;
    await post("/auth/passkey/register/finish", {
      state_id,
      label: label || null,
      credential: {
        id: cred.id,
        rawId: b64u.enc(cred.rawId),
        type: cred.type,
        extensions: cred.getClientExtensionResults ? cred.getClientExtensionResults() : {},
        response: {
          attestationObject: b64u.enc(res.attestationObject),
          clientDataJSON: b64u.enc(res.clientDataJSON),
          transports: res.getTransports ? res.getTransports() : undefined,
        },
      },
    });
    return "ok";
  } catch (e) {
    return friendly(e);
  }
}

export async function passkeyLogin() {
  if (!passkeySupported()) return "This browser doesn't support passkeys.";
  try {
    const { state_id, options } = await post("/auth/passkey/login/start");
    const pk = options.publicKey;
    pk.challenge = b64u.dec(pk.challenge);
    (pk.allowCredentials || []).forEach((c) => (c.id = b64u.dec(c.id)));
    const cred = await navigator.credentials.get({ publicKey: pk, mediation: "optional" });
    const res = cred.response;
    await post("/auth/passkey/login/finish", {
      state_id,
      credential: {
        id: cred.id,
        rawId: b64u.enc(cred.rawId),
        type: cred.type,
        extensions: cred.getClientExtensionResults ? cred.getClientExtensionResults() : {},
        response: {
          authenticatorData: b64u.enc(res.authenticatorData),
          clientDataJSON: b64u.enc(res.clientDataJSON),
          signature: b64u.enc(res.signature),
          userHandle: res.userHandle ? b64u.enc(res.userHandle) : null,
        },
      },
    });
    return "ok";
  } catch (e) {
    return friendly(e);
  }
}
