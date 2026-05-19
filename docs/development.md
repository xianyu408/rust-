# Development

## Environment

Required:

- Rust toolchain
- Verilator or Icarus Verilog

Optional:

- Yosys
- ngspice
- Node.js for MCP servers
- `OPENAI_API_KEY` for Rig/OpenAI generation

## Commands

```powershell
cargo fmt
cargo check --workspace
cargo run -p web-api
```

On Windows with the MSVC Rust target, run Cargo from the Visual Studio developer environment so `link.exe` resolves to MSVC, not MSYS2:

```powershell
$env:Path = [System.Environment]::GetEnvironmentVariable('Path','Machine') + ';' + [System.Environment]::GetEnvironmentVariable('Path','User')
cmd /c "`"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\Common7\Tools\VsDevCmd.bat`" -arch=x64 -host_arch=x64 && cargo check --workspace"
```

If EDA tools are installed through MSYS2, keep `C:\msys64\usr\bin` after Visual Studio paths. MSYS2 also ships a `link.exe`, and it must not be selected as the Rust MSVC linker.

## Environment variables

```powershell
$env:CHIP_AGENT_BIND = "127.0.0.1:8080"
$env:OPENAI_API_KEY = "..."
$env:CHIP_AGENT_MODEL = "gpt-4o"
```

Without `OPENAI_API_KEY`, the system uses the deterministic heuristic design agent.
