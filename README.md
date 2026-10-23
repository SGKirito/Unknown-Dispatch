# Unknown Dispatch

**Unknown Dispatch** is a lightweight, portable Windows utility that combines multiple internet connections into one faster, more resilient link using `Unknown Dispatch`. It bonds your adapters (e.g., Ethernet + Wi-Fi + USB Tethering) behind a local proxy and can route your entire system through it no external client and installer.

## Download

Grab the portable `Unknown Dispatch.exe` from the [Releases page](https://github.com/SGKirito/Unknown-Dispatch/releases).

## Features

- **Multi-Adapter Bonding**: select which network adapters to combine and give each one a weight.
- **System-Wide Proxy**: the Built-in mode configures the Windows System Proxy automatically, so the entire system's traffic goes through your bonded adapters.
- **External Modes**: Use your own proxy client.
- **Ironclad Failsafe**: the Windows System Proxy is restored on Stop, on Exit, on failure, and on the next launch after a crash — you never lose internet access.
- **Live Stats**: active/total connections and relayed traffic, plus a live log console.

## Proxy Modes

| Mode                               | Who it's for                | What it does                                                                                                                           |
| ---------------------------------- | --------------------------- | -------------------------------------------------------------------------------------------------------------------------------------- |
| **Built-in (System-wide)** default | Everyone                    | Runs the HTTP bridge next to the SOCKS5 listener and configures the Windows System Proxy for you. No external software needed.         |
| **External**                       | Users with their own client | Runs the SOCKS5 listener only and never touches the system proxy. Point your own client (browser, v2rayN, Clash, etc.) at it yourself. |

### How the built-in mode works

1. Click **Start** Unknown Dispatch binds your selected adapters and opens the SOCKS5 and HTTP listeners.
2. It waits until the HTTP proxy is actually accepting connections.
3. Only then does it enable the Windows System Proxy (localhost/LAN addresses bypassed), so a failed start never breaks your internet.
4. Click **Stop** (or exit the app) and everything shuts down and the system proxy is cleared; even if the app crashed, the next launch cleans up the stale setting.

## Usage

1. Open **Unknown Dispatch**.
2. **Listen IP / SOCKS port**: leave as default (`127.0.0.1:1080`) unless you have a custom setup.
3. **Proxy mode**: pick **Built-in (System-wide)** or **External**. In Built-in mode you can also set the HTTP port (default `8080`).
4. **Network interfaces**: check the adapters you want to combine and optionally give each one a weight.
5. Click **Start**. The status line shows the listeners and, in Built-in mode, when the system-wide proxy is on.
6. Click **Stop** to shut down and restore your standard internet settings.
