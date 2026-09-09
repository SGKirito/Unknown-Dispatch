# Dispatch Proxy Manager

![Platform: Windows](https://img.shields.io/badge/Platform-Windows-blue.svg)
![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)
![Framework: .NET 4.0+](https://img.shields.io/badge/.NET_Framework-4.0+-purple.svg)

**Dispatch Proxy Manager** is a modern, lightweight Windows GUI utility designed to manage multiple network connections (connection bonding/load balancing) using `dispatch`. It allows you to seamlessly combine multiple internet adapters (e.g., Ethernet + Wi-Fi + USB Tethering) and route them through a local proxy.

## Download

Grab the latest portable `DispatchProxyManager.exe` from the [Releases page](https://github.com/SGKirito/Dispatch-Proxy-Manager/releases).

## Features

- **Multi-Adapter Bonding**: select which network adapters to combine.
- **IPv4 & IPv6 Support**: Choose between IPv4 only or IPv4/IPv6 dual-stack on a per-adapter basis.
- **Two Proxy Client Modes**: built-in client and external client.
- **Ironclad Failsafe**: Forcefully resets the Windows System Proxy via the Registry on exit _and_ on the next launch after a crash, so you never lose internet access.
- **Modern UI**: Sleek dark mode design.

## Proxy Client Modes

| Mode                                  | Who it's for                | What it does                                                                                                                                                        |
| ------------------------------------- | --------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Built-in (System Proxy)** default | Everyone                    | Runs a built-in HTTP proxy on `127.0.0.1:10809` that forwards into dispatch's SOCKS5, and configures the Windows System Proxy for you. No external software needed. |
| **External Client**                   | Users with their own client | Launches your preferred proxy client (e.g., `v2rayN.exe`) after dispatch starts, exactly like older versions. You manage that client's settings yourself.           |

### How the built-in mode works

1. Click **Start Dispatch** dispatch binds your selected adapters and opens its SOCKS5 proxy.
2. The app waits until the backend is actually accepting connections.
3. Only then does it enable the Windows System Proxy (with `localhost`/LAN addresses bypassed), so a failed start never breaks your internet.
4. Click **Stop Dispatch** (or exit the app) and everything is shut down and the system proxy is cleared even if the app crashed, the next launch cleans up the stale setting.

## Usage

1. Open **Dispatch Proxy Manager**.
2. **Listen IP / Port**: Leave as default (`127.0.0.1:1080`) unless you have a custom setup.
3. **Proxy Client**: Pick a mode.
   - _Built-in_: just set the HTTP port (default `10809`) if you like.
   - _External_: click "Browse" and select your proxy client (e.g., `v2rayN.exe`).
4. **Network Adapters**: Check the "Use" box next to the internet connections you want to combine.
5. Click **Start Dispatch**. The app will spin up the local proxy, bind your adapters, start the chosen client and set the Windows System Proxy where applicable.
6. Click **Stop Dispatch** to safely shut down all processes and restore your standard internet settings.

## Building from source

No build script needed one command with the .NET Framework C# compiler (bundled with Windows) produces the portable executable:

```
%WINDIR%\Microsoft.NET\Framework64\v4.0.30319\csc.exe /nologo /target:winexe /out:DispatchProxyManager.exe /r:System.Web.Extensions.dll /win32icon:logo.ico /resource:dispatch.exe,dispatch.exe DispatchProxyManager.cs
```

Without the embedded `dispatch.exe` resource the app still works: it falls back to `dispatch` on the system PATH.
