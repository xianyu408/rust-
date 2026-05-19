# Five-stage implementation plan

## Phase 1: MVP

Implemented:

- Axum API with project, design, simulation, repair, job, artifact, and SSE endpoints.
- Rig-ready `HybridDesignAgent`.
- Claude/Anthropic-first design generation when `ANTHROPIC_API_KEY` is configured, with OpenAI fallback through `OPENAI_API_KEY`.
- RAG context injection through `DesignRequest.retrieved_context`.
- Offline deterministic RTL fallback for ALU/counter/default block.
- Verilator lint and simulation path.
- Icarus Verilog fallback.
- Yosys synthesis check plus gate-level Verilog, JSON, and BLIF netlist exports.
- Waveform artifact discovery and VCD text digest for structured analysis.
- KiCad 8+ schematic hand-off files under `reports/kicad/`.
- Structured verification analysis reports under `reports/`.
- ngspice batch hook.
- In-memory job state and event history.
- Repair analysis endpoint that reads failed simulator summaries from a source job.

Exit criteria:

- `cargo run -p web-api` starts the service.
- `/health` reports local EDA tool availability.
- A design job writes RTL/testbench artifacts and emits live events.

## Phase 2: Engineering

Next implementation:

- Add SQLite/PostgreSQL storage for `projects`, `jobs`, `job_events`, and `artifacts`.
- Store command manifests under `runs/{job_id}/manifest.json`.
- Store simulator reports under `reports/{job_id}/`.
- Add artifact versioning and immutable design snapshots.
- Make `/repair` produce and apply patch artifacts with explicit review gates.

## Phase 3: MCP

Next implementation:

- Load `config/mcp.json` into `McpSettings`.
- Start Context7 and Tavily MCP clients through Rig/rmcp.
- Register trusted documentation tools separately from external search tools.
- Require source URLs and timestamps for external search results.
- Cache search results per job to make runs reproducible.

## Phase 4: Chip flow

Next implementation:

- Add ngspice route for `.cir/.spice` simulation.
- Add OpenLane/OpenROAD job type for RTL-to-GDS experiments.
- Parse timing, area, power, DRC, and LVS reports.
- Add report summarization agent that references exact report files.

## Phase 5: Security

Next implementation:

- Run EDA commands in a locked workspace root.
- Apply process timeouts, output limits, memory limits, and CPU limits.
- Use containers or a restricted worker account for untrusted RTL/testbench execution.
- Deny absolute paths, parent directory traversal, and shell interpolation.
- Persist audit logs for every agent output, file write, and command execution.
