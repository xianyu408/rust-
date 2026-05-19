# API

Base URL: `http://127.0.0.1:8080`

## Health

```text
GET /health
```

Returns API health and detected local EDA tools.

## Create project

```text
POST /api/projects
```

```json
{
  "name": "alu-demo"
}
```

## Start design job

```text
POST /api/projects/{project_id}/design
```

```json
{
  "prompt": "设计一个 8-bit ALU，支持 add/sub/and/or/xor，生成 SystemVerilog 和 testbench",
  "language": "systemverilog",
  "target": "simulation",
  "max_repair_rounds": 2,
  "retrieved_context": [
    {
      "source": "internal-rag://alu-guidelines",
      "title": "ALU verification checklist",
      "content": "Use a self-checking testbench and cover arithmetic and bitwise operations."
    }
  ]
}
```

The job injects optional RAG context into the design prompt, writes generated files, runs simulator feedback, emits structured verification analysis, and writes report artifacts.

## Start simulation job

```text
POST /api/projects/{project_id}/simulate
```

```json
{
  "top": "alu8_tb",
  "synthesis_top": "alu8",
  "rtl_files": ["src/alu8.sv"],
  "testbench_files": ["tb/alu8_tb.sv"],
  "use_yosys": true,
  "generate_waveform": true,
  "generate_kicad": true
}
```

When `generate_waveform` is enabled, the runner searches `runs/` for VCD/FST artifacts and writes a waveform digest into `reports/verification_analysis.json`. When `generate_kicad` is enabled with Yosys, the runner writes gate-level Verilog, Yosys JSON/BLIF netlists, and KiCad 8+ schematic hand-off files under `reports/kicad/`.

## Start repair analysis job

```text
POST /api/projects/{project_id}/repair
```

```json
{
  "job_id": "source-design-or-simulation-job-id",
  "max_repair_rounds": 1
}
```

The repair job reads simulation summaries from the source job's event history and emits a structured repair suggestion.

## Job events

```text
GET /api/jobs/{job_id}/events
```

Server-Sent Events are emitted as `job_event` with JSON payloads:

```json
{
  "job_id": "...",
  "sequence": 1,
  "level": "info",
  "message": "Design job started",
  "at": "2026-05-15T00:00:00Z",
  "data": {}
}
```

## Artifacts

```text
GET /api/projects/{project_id}/artifacts
GET /api/projects/{project_id}/files/src/alu8.sv
```

Paths must be relative and cannot contain parent directory components.
