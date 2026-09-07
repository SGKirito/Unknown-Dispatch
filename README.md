# Dispatch Proxy Manager

![Platform: Windows](https://img.shields.io/badge/Platform-Windows-blue.svg)
![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)
![Framework: .NET 4.0+](https://img.shields.io/badge/.NET_Framework-4.0+-purple.svg)

**Dispatch Proxy Manager** is a modern, lightweight Windows GUI utility designed to manage multiple network connections (connection bonding/load balancing) using `dispatch`. It allows you to seamlessly combine multiple internet adapters (e.g., Ethernet + Wi-Fi + USB Tethering) and route them through a local proxy. 

It also orchestrates background proxy clients like **v2rayN**, automatically starting them when you connect, and safely cleaning up Windows System Proxy settings when you disconnect to ensure you never lose internet access.

## Features

- **Multi-Adapter Bonding**: Visually select which network adapters to combine.
- **IPv4 & IPv6 Support**: Choose between IPv4 only or IPv4/IPv6 dual-stack on a per-adapter basis.
- **Portable Single Executable**: Directly embeds `dispatch.exe` inside the application so you only need to distribute one file.
- **Proxy Client Orchestration**: Automatically launches your preferred proxy client (e.g., `v2rayN.exe`).
- **Ironclad Failsafe**: Gracefully closes proxy clients on exit and forcefully resets the Windows System Proxy via the Registry to prevent "no internet" bugs if the app crashes.
- **Modern UI**: Sleek dark mode design.

## Usage

1. Open **Dispatch Proxy Manager**.
2. **Listen IP / Port**: Leave as default (`127.0.0.1:1080`) unless you have a custom setup.
3. **Proxy Path**: Click "Browse" and select your proxy client (e.g., `v2rayN.exe`).
4. **Network Adapters**: Check the "Use" box next to the internet connections you want to combine.
5. Click **Start Dispatch**. The app will spin up the local proxy, bind your adapters, and launch your proxy client.
6. Click **Stop Dispatch** to safely shut down all processes and restore your standard internet settings.

## How to Build from Source

You don't need Visual Studio to compile this! You can build it using the C# compiler built directly into Windows.

1. Ensure `DispatchGUI.cs` and `dispatch.exe` are in the same folder.
2. Open Command Prompt (`cmd.exe`).
3. Run the following command to compile the GUI and embed `dispatch.exe` inside it:

```cmd
C:\Windows\Microsoft.NET\Framework\v4.0.30319\csc.exe /target:winexe /resource:dispatch.exe DispatchGUI.cs
