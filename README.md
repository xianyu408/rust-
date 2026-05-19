# Chip Agent System

Rust chip-development agent system built with Rig and Axum. The system accepts chip design requests, can inject retrieved RAG context into the design prompt, generates RTL/testbench artifacts, runs simulator feedback, exports verification reports, and streams validation logs back through a web API.

## Five-stage roadmap

1. MVP: Axum API, Rig-ready agent facade, Verilator/Icarus/Yosys command runner, project workspace, SSE job events.
2. Engineering: durable project/job storage, artifact index, synthesis reports, repeatable command manifests.
3. MCP: Context7/Tavily/internal-doc MCP clients through Rig + rmcp.
4. Chip flow: ngspice, OpenLane/OpenROAD, timing/area/power reports, waveform OCR analysis, and KiCad reverse-schematic hand-off.
5. Security: sandboxed EDA execution, path policy, CPU/memory/time limits, audit trails.

## Current implementation

This repository implements the phase-1 runtime and lays out concrete extension points for phases 2-5.

```text
crates/domain      Shared request/response models and job events
crates/eda-runner  Simulator command execution and result parsing
crates/agent-core  Rig-ready design/review/repair orchestration
crates/web-api     Axum HTTP API and SSE streaming
docs/              Architecture, API, phases, MCP, security notes
config/            MCP server example configuration
workspaces/        Generated project files and run artifacts
```

## Run

Install Rust and at least one simulator first:

```powershell
winget install Rustlang.Rustup
winget install verilator
```

Then:

```powershell
cargo run -p web-api
```

The API listens on `127.0.0.1:8080` by default.

Open the web UI:

```text
http://127.0.0.1:8080/
```

## Example

```powershell
$body = @{
  prompt = "设计一个 8-bit ALU，支持 add/sub/and/or/xor，生成 SystemVerilog 和 testbench"
  language = "systemverilog"
  target = "simulation"
  max_repair_rounds = 2
} | ConvertTo-Json

Invoke-RestMethod -Method Post -Uri http://127.0.0.1:8080/api/projects -Body '{}' -ContentType 'application/json'
Invoke-RestMethod -Method Post -Uri http://127.0.0.1:8080/api/projects/<project_id>/design -Body $body -ContentType 'application/json'
```

Stream logs:

```powershell
curl.exe http://127.0.0.1:8080/api/jobs/<job_id>/events
```
