# Architecture

The system separates user-facing orchestration from EDA execution. Axum owns HTTP, project state, job state, and streaming. Agent Core owns design generation, review, repair suggestions, and future MCP tool use. EDA Runner owns process execution for simulators and synthesis checks.

```text
Client
  |
  v
Axum web-api
  |-- project and job state
  |-- SSE event history and live stream
  |-- artifact read/list endpoints
  |
  v
agent-core
  |-- HybridDesignAgent
  |     |-- RigDesignAgent when ANTHROPIC_API_KEY or OPENAI_API_KEY is configured
  |     `-- HeuristicDesignAgent fallback
  |-- HeuristicRepairAgent
  `-- DesignOrchestrator
  |
  v
eda-runner
  |-- Verilator lint and binary simulation
  |-- Icarus Verilog fallback
  |-- Yosys synthesis check and netlist export
  |-- KiCad 8+ reverse schematic hand-off artifacts
  |-- Waveform digest and structured verification prompt
  `-- ngspice batch hook
```

## Design flow

1. `POST /api/projects` creates a project workspace.
2. `POST /api/projects/{id}/design` queues a design job.
3. The background job asks `HybridDesignAgent` for a design package.
4. Optional RAG snippets from `retrieved_context` are injected into the Claude/OpenAI prompt as supporting context.
5. Artifacts are written under the project workspace.
6. `EdaRunner` runs digital simulator feedback, waveform capture lookup, Yosys netlist export, and KiCad hand-off generation.
7. The runner writes `reports/verification_analysis.json` and `.md`, including a structured prompt for downstream log/waveform reasoning.
8. The feedback agent classifies failures and emits repair guidance.
9. Job events are persisted in memory and streamed through SSE.

## Agent policy

Agent output is treated as a proposal. Simulator and synthesis checks are the acceptance gate. Any claim that a design is usable must reference command output, exit code, and generated artifacts.

## Current persistence model

Phase 1 stores project and job metadata in memory and writes artifacts to `workspaces/{project_id}`. Phase 2 should move metadata and event history into SQLite or PostgreSQL.
