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

function friendly(e, login) {
  // On sign-in this usually means the browser had no passkey for us (it offered a QR code).
  if (e && e.name === "NotAllowedError") return login ? "not_here" : "Cancelled, or the device didn't confirm.";
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
    // A discoverable passkey on this device (Touch ID / Face ID / Windows Hello). On a
    // Mac, Safari and Chrome can keep it in iCloud Keychain, which syncs to the phone.
    pk.authenticatorSelection = { ...(pk.authenticatorSelection || {}), residentKey: "required", requireResidentKey: true, userVerification: "required" };
    pk.hints = ["client-device"];
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
    markHere();
    return "ok";
  } catch (e) {
    return friendly(e);
  }
}

// Remember that this browser holds a Juggernaught passkey (to stop offering one).
function markHere() {
  try {
    localStorage.setItem("jugg-passkey-here", "1");
  } catch {}
}

export function passkeyHere() {
  try {
    return localStorage.getItem("jugg-passkey-here") === "1";
  } catch {
    return false;
  }
}

// `anyDevice`: also try passkeys this browser doesn't know about (a phone via QR code,
// another keychain). Without it, a browser that never made or used a Juggernaught
// passkey skips the system dialog (which would only offer a QR code).
export async function passkeyLogin(anyDevice) {
  if (!passkeySupported()) return "This browser doesn't support passkeys.";
  if (!anyDevice && !passkeyHere()) return "not_here";
  try {
    const { state_id, options } = await post("/auth/passkey/login/start");
    const pk = options.publicKey;
    pk.challenge = b64u.dec(pk.challenge);
    (pk.allowCredentials || []).forEach((c) => (c.id = b64u.dec(c.id)));
    pk.hints = ["client-device"];
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
    markHere();
    return "ok";
  } catch (e) {
    return friendly(e, true);
  }
}
