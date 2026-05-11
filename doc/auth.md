# Tango Bridge Authentication

Tango Bridge supports two authentication modes for WebSocket connections.

- **PIN authentication** – always active; a 6-digit PIN is generated on first run.
- **Public-key (Ed25519) authentication** – opt-in; one or more public keys are
  configured at build time and/or at runtime.

A WebSocket connection is accepted when **either** mode produces a valid credential.

---

## 1. Creating an Ed25519 Key Pair

Use Node.js (20+) or the OpenSSL CLI to generate a key pair.

### Using Node.js

```js
import { generateKeyPairSync } from "node:crypto";
import { Buffer } from "node:buffer";

const { publicKey, privateKey } = generateKeyPairSync("ed25519", {
  publicKeyEncoding: { type: "spki", format: "der" },
  privateKeyEncoding: { type: "pkcs8", format: "der" },
});

// The raw 32-byte public key is the last 32 bytes of the DER-encoded SPKI blob.
const rawPublicKey = publicKey.slice(-32);
console.log("Public key (base64):", rawPublicKey.toString("base64"));
console.log("Private key (pkcs8 DER base64):", privateKey.toString("base64"));
```

### Using OpenSSL

```sh
# Generate private key in PEM format
openssl genpkey -algorithm ed25519 -out private.pem

# Derive the public key
openssl pkey -in private.pem -pubout -out public.pem

# Extract the raw 32-byte public key (last 32 bytes of DER SPKI)
openssl pkey -in private.pem -pubout -outform DER \
  | tail -c 32 \
  | base64
```

Store the private key (PEM or DER) securely on the server that will sign connection tokens. The raw 32-byte base64 string is the value you supply in the next step.

---

## 2. Configuring Public Keys

### Build-time (embedded permanently in the binary)

Pass one or more raw 32-byte public keys (standard base64, comma-separated) via
`TANGO_BRIDGE_PUBLIC_KEY` at build time:

```sh
# Single key — macOS / Linux
TANGO_BRIDGE_PUBLIC_KEY="<key1-base64>" cargo build --release

# Multiple keys — macOS / Linux
TANGO_BRIDGE_PUBLIC_KEY="<key1-base64>,<key2-base64>" cargo build --release

# Windows (PowerShell)
$env:TANGO_BRIDGE_PUBLIC_KEY = "<key1-base64>,<key2-base64>"
cargo build --release
```

The keys are baked into the binary with `option_env!`.

### Runtime (read when the server starts)

Set the same `TANGO_BRIDGE_PUBLIC_KEY` environment variable when **running** the
binary (comma-separated). These keys are in addition to any build-time keys:

```sh
TANGO_BRIDGE_PUBLIC_KEY="<key1-base64>" ./tango-bridge
```

If neither build-time nor runtime keys are configured, public-key authentication
is disabled and only PIN authentication is active.

---

## 3. Authentication Flow for Public-Key Auth

The public-key flow uses a **challenge–response** scheme to prevent replay attacks.
Each challenge expires after **one hour** but may be reused for multiple WebSocket
connections within that window.

```
Client (browser)         Your Server              Tango Bridge
  │                           │                        │
  │  GET /bridge/ping         │                        │
  │───────────────────────────────────────────────────>│
  │                           │                        │  generate challenge
  │  { version, challenge, publicKeys }                │
  │<───────────────────────────────────────────────────│
  │                           │                        │
  │  POST /token              │                        │
  │  { challenge }            │                        │
  │──────────────────────────>│                        │
  │                           │  sign(challenge)       │
  │                           │  → token               │
  │  { token }                │                        │
  │<──────────────────────────│                        │
  │                           │                        │
  │  WebSocket /bridge/?challenge=…&token=…            │
  │───────────────────────────────────────────────────>│  verify token
  │  (upgrade)                                         │
  │<───────────────────────────────────────────────────│
```

1. **Fetch a challenge** from `GET /bridge/ping`:
   ```json
   { "version": "0.3.0", "challenge": "dGFuZ29icmlkZ2U", "publicKeys": ["<base64>"] }
   ```
   Check `publicKeys` to confirm your key is trusted before proceeding.
2. **Send the challenge to your server** to sign it (private key stays server-side).
3. **Connect** the WebSocket with both `challenge` and `token` query parameters.

> Challenges expire after one hour. Your application should track when the current
> challenge was obtained and request a fresh one before expiry (e.g. every 55 minutes).

---

## 4. Generating Signatures on the Server Side

The signature is an Ed25519 signature of the **challenge string** (UTF-8 bytes) received
from `/bridge/ping`.

> **Important:** The private key must **never** be embedded in client-side (browser)
> code. Websites must call a backend API endpoint to obtain a fresh token, as shown
> in the examples below.

### Node.js (server-side API endpoint)

```js
// token-api.mjs  –  run with: node token-api.mjs
import { createSign } from "node:crypto";
import { readFileSync } from "node:fs";
import { createServer } from "node:http";

// Load the private key (PEM file kept on the server, never shipped to clients)
const privateKeyPem = readFileSync("private.pem");

function generateToken(challenge) {
  const sign = createSign("Ed25519");
  sign.update(challenge);
  sign.end();
  // The bridge accepts URL-safe base64 (no padding) or standard base64
  return sign.sign(privateKeyPem).toString("base64url");
}

const server = createServer((req, res) => {
  const url = new URL(req.url, "http://localhost");

  if (url.pathname === "/token" && req.method === "POST") {
    // The browser sends the challenge it obtained from /bridge/ping
    let body = "";
    req.on("data", (chunk) => { body += chunk; });
    req.on("end", () => {
      const { challenge } = JSON.parse(body);
      res.setHeader("Content-Type", "application/json");
      res.end(JSON.stringify({ token: generateToken(challenge) }));
    });
  } else {
    res.writeHead(404);
    res.end();
  }
});

server.listen(3000, () => console.log("Token server running on http://localhost:3000"));
```

### WebCrypto API (server-side, e.g. Deno / Cloudflare Workers / Next.js Route Handler)

```ts
// token-handler.ts  –  Next.js App Router example (runs server-side only)
import { NextResponse } from "next/server";

// Store the private key as a base64-encoded PKCS#8 DER blob in an env variable.
// Never expose this to the client.
const PRIVATE_KEY_B64 = process.env.TANGO_BRIDGE_PRIVATE_KEY!;

let cachedKey: CryptoKey | undefined;

async function getPrivateKey(): Promise<CryptoKey> {
  if (cachedKey) return cachedKey;
  const der = Uint8Array.from(atob(PRIVATE_KEY_B64), (c) => c.charCodeAt(0));
  cachedKey = await crypto.subtle.importKey(
    "pkcs8",
    der,
    { name: "Ed25519" },
    false,
    ["sign"],
  );
  return cachedKey;
}

function toBase64Url(buf: ArrayBuffer): string {
  return btoa(String.fromCharCode(...new Uint8Array(buf)))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}

// The browser POSTs the challenge it obtained from /bridge/ping
export async function POST(request: Request) {
  const { challenge } = await request.json();

  const key = await getPrivateKey();
  const sig = await crypto.subtle.sign(
    "Ed25519",
    key,
    new TextEncoder().encode(challenge),
  );

  return NextResponse.json({ token: toBase64Url(sig) });
}
```

---

## 5. Creating WebSocket Connections from the Browser

All WebSocket connections are opened from browser JavaScript. The credential is passed
as query string parameters on the WebSocket URL.

### Connection with a signature token

The browser fetches a challenge from Tango Bridge, sends it to your backend to sign,
then opens the WebSocket. The challenge is reusable for the full hour, so cache it
and refresh proactively (e.g. every 55 minutes).

```js
let authCache = null; // { challenge, token, expiresAt }

async function getAuth() {
  const now = Date.now();
  // Refresh 5 minutes before the challenge expires
  if (authCache && authCache.expiresAt - now > 5 * 60 * 1000) {
    return authCache;
  }

  // 1. Fetch a challenge from Tango Bridge
  const pingRes = await fetch("http://localhost:15037/bridge/ping");
  const { challenge, challengeTtl } = await pingRes.json();

  // 2. Ask your backend to sign it (private key never leaves the server)
  const tokenRes = await fetch("https://your-server.example.com/token", {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ challenge }),
  });
  const { token } = await tokenRes.json();

  authCache = { challenge, token, expiresAt: now + challengeTtl * 1000 };
  return authCache;
}

async function connectWithToken() {
  const { challenge, token } = await getAuth();

  const ws = new WebSocket(
    `ws://localhost:15037/bridge/?challenge=${encodeURIComponent(challenge)}&token=${encodeURIComponent(token)}`
  );

  ws.binaryType = "arraybuffer";

  ws.addEventListener("open", () => {
    console.log("Connected (token auth)");
  });

  ws.addEventListener("message", (event) => {
    // event.data is an ArrayBuffer containing raw ADB protocol bytes
    handleAdbData(new Uint8Array(event.data));
  });

  ws.addEventListener("close", (event) => {
    console.log("Connection closed:", event.code, event.reason);
  });

  return ws;
}
```

### Connection with a PIN

The user reads the PIN from the Tango Bridge tray icon menu and enters it in your
Web app. Check that Tango Bridge is running first by calling `/bridge/ping`.

```js
async function connectWithPin() {
  // 1. Check that Tango Bridge is running
  let bridgeRunning = false;
  try {
    const res = await fetch("http://localhost:15037/bridge/ping");
    bridgeRunning = res.ok;
  } catch (_) {
    // Bridge not reachable
  }

  if (!bridgeRunning) {
    alert("Tango Bridge is not running. Please start it and try again.");
    return null;
  }

  // 2. Prompt the user to enter the PIN shown in the tray icon menu
  const pin = prompt("Enter the PIN shown in the Tango Bridge tray menu:");
  if (!pin) return null;

  // 3. Open the WebSocket connection
  const ws = new WebSocket(`ws://localhost:15037/bridge/?pin=${encodeURIComponent(pin)}`);

  ws.binaryType = "arraybuffer";

  ws.addEventListener("open", () => {
    console.log("Connected (PIN auth)");
  });

  ws.addEventListener("message", (event) => {
    handleAdbData(new Uint8Array(event.data));
  });

  ws.addEventListener("close", (event) => {
    console.log("Connection closed:", event.code, event.reason);
  });

  return ws;
}
```

### Sending and receiving ADB data

Once connected, the WebSocket carries raw ADB protocol binary messages:

```js
// Send data to ADB
function sendToAdb(ws, data /* Uint8Array */) {
  if (ws.readyState === WebSocket.OPEN) {
    ws.send(data);
  }
}

// Receive data from ADB (registered in the message handler above)
function handleAdbData(data /* Uint8Array */) {
  // Parse ADB protocol packets here
}
```

---

## Quick Reference

### `/bridge/ping` endpoint

`GET http://localhost:15037/bridge/ping` — returns:

```json
{
  "version": "0.3.0",
  "challenge": "<22-char URL-safe base64>",
  "publicKeys": ["<base64-key1>", "<base64-key2>"],
  "challengeTtl": 3600
}
```

- `challenge` may be reused for multiple connections until it expires.
- `challengeTtl` is the challenge lifetime in seconds; refresh the challenge before this many seconds have elapsed (e.g. after 55 minutes to be safe).
- `publicKeys` lists the currently trusted public keys (empty array when no keys are configured).

### WebSocket query parameters

| Parameter   | Value | Notes |
|-------------|-------|-------|
| `challenge` | Challenge string from `/bridge/ping` | Required for public-key auth |
| `token`     | URL-safe base64 (no padding) or standard base64 Ed25519 signature of the challenge | Required for public-key auth |
| `pin`       | 6-digit zero-padded string, e.g. `042817` | Found in `~/.android/tango-bridge.pin` and the tray icon menu |
