# Tango Bridge Authentication

Tango Bridge supports two authentication modes for WebSocket connections.

- **PIN authentication** – always active; a 6-digit PIN is generated on first run.
- **Public-key (Ed25519) authentication** – opt-in; a public key is embedded at build time.

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

Store the private key (PEM or DER) securely on the server that will sign connection tokens. The raw 32-byte base64 string is the value you supply to `cargo build` in the next step.

---

## 2. Building with an Embedded Public Key

Pass the raw 32-byte public key (standard base64) via the `TANGO_BRIDGE_PUBLIC_KEY` environment variable at build time:

```sh
# macOS / Linux
TANGO_BRIDGE_PUBLIC_KEY="<base64-encoded-32-byte-public-key>" cargo build --release

# Windows (PowerShell)
$env:TANGO_BRIDGE_PUBLIC_KEY = "<base64-encoded-32-byte-public-key>"
cargo build --release
```

The key is baked into the binary with `option_env!`. If the variable is not set, public-key authentication is disabled and only PIN authentication is active.

---

## 3. Generating Signatures on the Server Side

The signature is an Ed25519 signature of the **current UTC date string** in `YYYY-MM-DD` format (e.g. `"2024-07-15"`). The server verifies today's date as well as the previous and next day (±1 day) to tolerate clock skew.

> **Important:** The private key must **never** be embedded in client-side (browser) code. Websites must call a backend API endpoint to obtain a fresh token, as shown in the examples below.

### Node.js (server-side API endpoint)

```js
// token-api.mjs  –  run with: node token-api.mjs
import { createSign } from "node:crypto";
import { readFileSync } from "node:fs";
import { createServer } from "node:http";

// Load the private key (PEM file kept on the server, never shipped to clients)
const privateKeyPem = readFileSync("private.pem");

function getUtcDateString() {
  return new Date().toISOString().slice(0, 10); // "YYYY-MM-DD"
}

function generateToken() {
  const date = getUtcDateString();
  const sign = createSign("Ed25519");
  sign.update(date);
  sign.end();
  // The bridge accepts URL-safe base64 (no padding) or standard base64
  return sign.sign(privateKeyPem).toString("base64url");
}

const server = createServer((req, res) => {
  if (req.url === "/token") {
    res.setHeader("Content-Type", "application/json");
    res.end(JSON.stringify({ token: generateToken() }));
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

function getUtcDateString(): string {
  return new Date().toISOString().slice(0, 10); // "YYYY-MM-DD"
}

function toBase64Url(buf: ArrayBuffer): string {
  return btoa(String.fromCharCode(...new Uint8Array(buf)))
    .replace(/\+/g, "-")
    .replace(/\//g, "_")
    .replace(/=+$/, "");
}

export async function GET() {
  const key = await getPrivateKey();
  const date = getUtcDateString();
  const sig = await crypto.subtle.sign(
    "Ed25519",
    key,
    new TextEncoder().encode(date),
  );
  return NextResponse.json({ token: toBase64Url(sig) });
}
```

---

## 4. Creating WebSocket Connections from the Browser

All WebSocket connections are opened from browser JavaScript. The credential is passed as a query string parameter on the WebSocket URL.

### Connection with a signature token

The client first fetches a fresh token from your server-side API, then opens the WebSocket:

```js
async function connectWithToken() {
  // 1. Fetch a short-lived token from your backend (private key stays on the server)
  const res = await fetch("https://your-server.example.com/token");
  const { token } = await res.json();

  // 2. Open the WebSocket connection to Tango Bridge
  const ws = new WebSocket(`ws://localhost:15037/bridge/?token=${token}`);

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

Display the PIN to the user (they read it from the Tango Bridge tray menu) and pass it directly:

```js
function connectWithPin(pin) {
  const ws = new WebSocket(`ws://localhost:15037/bridge/?pin=${pin}`);

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

| Parameter | Value | Notes |
|-----------|-------|-------|
| `token`   | URL-safe base64 (no padding) or standard base64 Ed25519 signature of `YYYY-MM-DD` | Requires public key embedded at build time |
| `pin`     | 6-digit zero-padded string, e.g. `042817` | Found in `~/.android/tango-bridge.pin` and the tray icon menu |
