# Osuwapp

Desktop companion for osu! stable with a restrained native-feeling UI and live PP display.

## Stack

- React 19 + Vite for the interface
- Tauri 2 for the Windows desktop shell
- `rosu-memory-lib` for reading `osu!.exe`
- `rosu-pp` for PP calculation

## Windows build commands

This project includes local wrappers that boot Visual Studio's developer environment before running Rust or Tauri commands. Use these from a normal PowerShell prompt:

```powershell
npm install
npm run tauri:dev
```

Production build:

```powershell
npm run tauri:build
```

If you want the Rust-side compile check only:

```powershell
npm run cargo:check:win
```

## Notes

- Live memory reading currently targets `osu!.exe` (stable).
- If the game is not running, the UI falls back to a mock preview state instead of an empty window.
- If Visual Studio C++ tools are installed in a different location, update the paths in [scripts/Invoke-VsDevCommand.ps1](scripts/Invoke-VsDevCommand.ps1) and [scripts/Set-VsDevEnvironment.ps1](scripts/Set-VsDevEnvironment.ps1).
