use anyhow::{anyhow, Context, Result};
use domain::{
    ArtifactKind, RunArtifact, SimulationSummary, VerificationAnalysis, WaveformDigest,
    WaveformSignal,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{process::Command, time::timeout};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimulationPlan {
    pub project_root: PathBuf,
    pub top: Option<String>,
    pub synthesis_top: Option<String>,
    pub rtl_files: Vec<PathBuf>,
    pub testbench_files: Vec<PathBuf>,
    pub use_yosys: bool,
    pub generate_waveform: bool,
    pub generate_kicad: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EdaTool {
    Verilator,
    Icarus,
    Yosys,
    Ngspice,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandSpec {
    pub tool: EdaTool,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolOutput {
    pub spec: CommandSpec,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolAvailability {
    pub verilator: bool,
    pub iverilog: bool,
    pub yosys: bool,
    pub ngspice: bool,
    pub kicad_cli: bool,
    pub tesseract: bool,
}

#[derive(Debug, Clone)]
pub struct EdaRunner {
    pub default_timeout: Duration,
}

impl Default for EdaRunner {
    fn default() -> Self {
        Self {
            default_timeout: Duration::from_secs(120),
        }
    }
}

impl EdaRunner {
    pub async fn availability(&self) -> ToolAvailability {
        ToolAvailability {
            verilator: command_exists("verilator_bin").await || command_exists("verilator").await,
            iverilog: command_exists("iverilog").await,
            yosys: command_exists("yosys").await,
            ngspice: command_exists("ngspice_con").await || command_exists("ngspice").await,
            kicad_cli: command_exists("kicad-cli").await,
            tesseract: command_exists("tesseract").await,
        }
    }

    pub async fn run_digital_simulation(
        &self,
        plan: &SimulationPlan,
    ) -> Result<Vec<SimulationSummary>> {
        validate_paths(plan)?;

        let availability = self.availability().await;
        let mut summaries = Vec::new();

        if availability.iverilog {
            summaries.push(self.icarus_sim(plan).await?);
        } else if availability.verilator {
            let lint = self.verilator_lint(plan).await?;
            let lint_passed = lint.passed;
            summaries.push(lint);

            if lint_passed {
                let build = self.verilator_binary(plan).await?;
                let build_passed = build.passed;
                summaries.push(build);

                if build_passed {
                    summaries.push(self.verilator_run(plan).await?);
                }
            }
        } else {
            return Err(anyhow!(
                "未找到数字仿真器。请安装 Verilator 或 Icarus Verilog。"
            ));
        }

        if plan.use_yosys {
            if availability.yosys {
                summaries.push(self.yosys_synth_check(plan).await?);
            } else {
                summaries.push(SimulationSummary {
                    passed: false,
                    tool: "yosys".to_string(),
                    command: "yosys".to_string(),
                    exit_code: None,
                    stdout: String::new(),
                    stderr: "已请求 Yosys，但在 PATH 中未找到。".to_string(),
                    artifacts: Vec::new(),
                    analysis: None,
                });
            }
        }

        self.write_verification_analysis(plan, &mut summaries)
            .await?;
        Ok(summaries)
    }

    pub async fn run_spice_batch(
        &self,
        project_root: &Path,
        netlist: &Path,
    ) -> Result<SimulationSummary> {
        let spec = CommandSpec {
            tool: EdaTool::Ngspice,
            program: resolve_program(&["ngspice_con", "ngspice"]).await?,
            args: vec!["-b".to_string(), normalize_arg(netlist)],
            cwd: project_root.to_path_buf(),
            timeout_secs: self.default_timeout.as_secs(),
        };

        let output = self.run_command(spec).await?;
        Ok(output.into_summary())
    }

    async fn verilator_lint(&self, plan: &SimulationPlan) -> Result<SimulationSummary> {
        let mut args = vec!["--lint-only".to_string(), "-Wall".to_string()];
        args.extend(plan.rtl_files.iter().map(|path| normalize_arg(path)));
        args.extend(plan.testbench_files.iter().map(|path| normalize_arg(path)));

        let spec = CommandSpec {
            tool: EdaTool::Verilator,
            program: resolve_program(&["verilator_bin", "verilator"]).await?,
            args,
            cwd: plan.project_root.clone(),
            timeout_secs: self.default_timeout.as_secs(),
        };

        Ok(self.run_command(spec).await?.into_summary())
    }

    async fn verilator_binary(&self, plan: &SimulationPlan) -> Result<SimulationSummary> {
        let obj_dir = PathBuf::from("runs").join("verilator_obj");
        tokio::fs::create_dir_all(plan.project_root.join("runs")).await?;

        let mut args = vec![
            "--binary".to_string(),
            "--timing".to_string(),
            "--Mdir".to_string(),
            normalize_arg(&obj_dir),
        ];
        if plan.generate_waveform {
            args.push("--trace".to_string());
        }
        args.extend(plan.rtl_files.iter().map(|path| normalize_arg(path)));
        args.extend(plan.testbench_files.iter().map(|path| normalize_arg(path)));
        if let Some(top) = &plan.top {
            args.push("--top-module".to_string());
            args.push(top.clone());
        }

        let spec = CommandSpec {
            tool: EdaTool::Verilator,
            program: resolve_program(&["verilator_bin", "verilator"]).await?,
            args,
            cwd: plan.project_root.clone(),
            timeout_secs: self.default_timeout.as_secs(),
        };

        Ok(self.run_command(spec).await?.into_summary())
    }

    async fn verilator_run(&self, plan: &SimulationPlan) -> Result<SimulationSummary> {
        let executable = self.verilator_executable(plan).await?;
        let spec = CommandSpec {
            tool: EdaTool::Verilator,
            program: normalize_arg(&executable),
            args: Vec::new(),
            cwd: plan.project_root.clone(),
            timeout_secs: self.default_timeout.as_secs(),
        };

        let mut summary = self.run_command(spec).await?.into_summary();
        if plan.generate_waveform {
            summary
                .artifacts
                .extend(self.waveform_artifacts(plan).await?);
        }
        Ok(summary)
    }

    async fn verilator_executable(&self, plan: &SimulationPlan) -> Result<PathBuf> {
        let top = plan
            .top
            .as_deref()
            .or_else(|| {
                plan.testbench_files
                    .first()
                    .and_then(|path| path.file_stem())
                    .and_then(|stem| stem.to_str())
            })
            .or_else(|| {
                plan.rtl_files
                    .first()
                    .and_then(|path| path.file_stem())
                    .and_then(|stem| stem.to_str())
            })
            .ok_or_else(|| anyhow!("cannot infer Verilator executable name"))?;

        let obj_dir = PathBuf::from("runs").join("verilator_obj");
        let mut candidates = vec![obj_dir.join(format!("V{top}"))];
        candidates.push(obj_dir.join(format!("V{top}.exe")));

        for candidate in candidates {
            if tokio::fs::metadata(plan.project_root.join(&candidate))
                .await
                .is_ok()
            {
                return Ok(candidate);
            }
        }

        Err(anyhow!(
            "Verilator executable for top module {top} was not found"
        ))
    }

    async fn icarus_sim(&self, plan: &SimulationPlan) -> Result<SimulationSummary> {
        let out_file = PathBuf::from("runs").join("sim.vvp");
        tokio::fs::create_dir_all(plan.project_root.join("runs")).await?;

        let mut compile_args = vec![
            "-g2012".to_string(),
            "-o".to_string(),
            normalize_arg(&out_file),
        ];
        compile_args.extend(plan.rtl_files.iter().map(|path| normalize_arg(path)));
        compile_args.extend(plan.testbench_files.iter().map(|path| normalize_arg(path)));

        let compile = self
            .run_command(CommandSpec {
                tool: EdaTool::Icarus,
                program: "iverilog".to_string(),
                args: compile_args,
                cwd: plan.project_root.clone(),
                timeout_secs: self.default_timeout.as_secs(),
            })
            .await?;

        if !compile.passed() {
            return Ok(compile.into_summary());
        }

        let run = self
            .run_command(CommandSpec {
                tool: EdaTool::Icarus,
                program: "vvp".to_string(),
                args: vec![normalize_arg(&out_file)],
                cwd: plan.project_root.clone(),
                timeout_secs: self.default_timeout.as_secs(),
            })
            .await?;

        let mut summary = run.into_summary();
        if plan.generate_waveform {
            summary
                .artifacts
                .extend(self.waveform_artifacts(plan).await?);
        }
        Ok(summary)
    }

    async fn yosys_synth_check(&self, plan: &SimulationPlan) -> Result<SimulationSummary> {
        tokio::fs::create_dir_all(plan.project_root.join("reports")).await?;
        let top = plan
            .synthesis_top
            .as_deref()
            .or_else(|| {
                plan.rtl_files
                    .first()
                    .and_then(|path| path.file_stem())
                    .and_then(|stem| stem.to_str())
            })
            .unwrap_or("design");
        let top = safe_identifier(top, "design");
        let gate_verilog = PathBuf::from("reports").join(format!("{top}_gate.v"));
        let netlist_json = PathBuf::from("reports").join(format!("{top}_netlist.json"));
        let netlist_blif = PathBuf::from("reports").join(format!("{top}_netlist.blif"));

        let mut script = String::new();
        for file in &plan.rtl_files {
            script.push_str(&format!("read_verilog -sv {}; ", normalize_arg(file)));
        }
        script.push_str(&format!("hierarchy -check -top {}; ", top));
        script.push_str("proc; opt; techmap; opt; check; stat; ");
        script.push_str(&format!(
            "write_verilog -noattr {}; ",
            normalize_arg(&gate_verilog)
        ));
        script.push_str(&format!("write_json {}; ", normalize_arg(&netlist_json)));
        script.push_str(&format!("write_blif {}; ", normalize_arg(&netlist_blif)));

        let spec = CommandSpec {
            tool: EdaTool::Yosys,
            program: "yosys".to_string(),
            args: vec!["-p".to_string(), script],
            cwd: plan.project_root.clone(),
            timeout_secs: self.default_timeout.as_secs(),
        };

        let mut summary = self.run_command(spec).await?.into_summary();
        if summary.passed {
            summary.artifacts.extend(
                existing_artifacts(
                    plan,
                    [
                        (
                            gate_verilog.clone(),
                            ArtifactKind::Netlist,
                            "Yosys 门级 Verilog 网表",
                        ),
                        (
                            netlist_json.clone(),
                            ArtifactKind::Netlist,
                            "用于后续原理图生成的 Yosys JSON 网表",
                        ),
                        (
                            netlist_blif.clone(),
                            ArtifactKind::Netlist,
                            "用于 EDA 交换的 Yosys BLIF 网表",
                        ),
                    ],
                )
                .await?,
            );

            summary.artifacts.extend(
                self.write_gate_diagram_assets(plan, &top, &netlist_json)
                    .await?,
            );

            if plan.generate_kicad {
                summary.artifacts.extend(
                    self.write_kicad_assets(plan, &top, &gate_verilog, &netlist_json)
                        .await?,
                );
            }
        }

        Ok(summary)
    }

    async fn waveform_artifacts(&self, plan: &SimulationPlan) -> Result<Vec<RunArtifact>> {
        let files = find_relative_files(
            &plan.project_root,
            &[PathBuf::from("runs")],
            &["vcd", "fst"],
        )
        .await?;
        Ok(files
            .into_iter()
            .map(|path| RunArtifact {
                path: normalize_arg(&path),
                kind: ArtifactKind::Waveform,
                description: "仿真器波形捕获".to_string(),
            })
            .collect())
    }

    async fn waveform_digest(&self, plan: &SimulationPlan) -> Result<Option<WaveformDigest>> {
        let files =
            find_relative_files(&plan.project_root, &[PathBuf::from("runs")], &["vcd"]).await?;
        let Some(path) = files.first() else {
            return Ok(None);
        };

        let bytes = tokio::fs::read(plan.project_root.join(path)).await?;
        let limit = bytes.len().min(256 * 1024);
        let text = String::from_utf8_lossy(&bytes[..limit]);
        Ok(Some(parse_vcd_digest(path, &text)))
    }

    async fn write_gate_diagram_assets(
        &self,
        plan: &SimulationPlan,
        top: &str,
        netlist_json: &Path,
    ) -> Result<Vec<RunArtifact>> {
        let json_text = tokio::fs::read_to_string(plan.project_root.join(netlist_json)).await?;
        let diagram = parse_yosys_gate_diagram(top, &json_text)?;
        let dot = PathBuf::from("reports").join(format!("{top}_gate.dot"));
        let svg = PathBuf::from("reports").join(format!("{top}_gate.svg"));
        let markdown = PathBuf::from("reports").join(format!("{top}_gate_diagram.md"));

        tokio::fs::write(
            plan.project_root.join(&dot),
            gate_diagram_dot(&diagram).as_bytes(),
        )
        .await?;
        tokio::fs::write(
            plan.project_root.join(&svg),
            gate_diagram_svg(&diagram).as_bytes(),
        )
        .await?;
        tokio::fs::write(
            plan.project_root.join(&markdown),
            gate_diagram_markdown(&diagram, &svg, &dot).as_bytes(),
        )
        .await?;

        existing_artifacts(
            plan,
            [
                (svg, ArtifactKind::Diagram, "Yosys 门级 SVG 图表"),
                (dot, ArtifactKind::Diagram, "Yosys 门级 DOT 图表"),
                (markdown, ArtifactKind::Report, "门级图表摘要"),
            ],
        )
        .await
    }

    async fn write_kicad_assets(
        &self,
        plan: &SimulationPlan,
        top: &str,
        gate_verilog: &Path,
        netlist_json: &Path,
    ) -> Result<Vec<RunArtifact>> {
        let kicad_dir = PathBuf::from("reports").join("kicad");
        tokio::fs::create_dir_all(plan.project_root.join(&kicad_dir)).await?;

        let schematic = kicad_dir.join(format!("{top}_reverse.kicad_sch"));
        let manifest = kicad_dir.join(format!("{top}_kicad_import_manifest.json"));
        let script = kicad_dir.join(format!("generate_{top}_kicad_sch.ps1"));

        tokio::fs::write(
            plan.project_root.join(&schematic),
            kicad_schematic(top, gate_verilog, netlist_json).as_bytes(),
        )
        .await?;

        let manifest_json = json!({
            "top": top,
            "flow": "yosys_gate_netlist_to_kicad8_reverse_schematic_assets",
            "inputs": {
                "gate_verilog": normalize_arg(gate_verilog),
                "yosys_json": normalize_arg(netlist_json)
            },
            "outputs": {
                "schematic": normalize_arg(&schematic)
            },
            "notes": [
                "此文件是用于 KiCad 8+ 原理图生成的自动化交接文件。",
                "生成的 .kicad_sch 故意保持极简，仅引用综合后的网表产物。",
                "未来的导入器可以用来自 Yosys JSON 图的门级符号替换这个占位图纸。"
            ]
        });
        tokio::fs::write(
            plan.project_root.join(&manifest),
            serde_json::to_vec_pretty(&manifest_json)?,
        )
        .await?;

        tokio::fs::write(
            plan.project_root.join(&script),
            kicad_script(top, &schematic, &manifest).as_bytes(),
        )
        .await?;

        existing_artifacts(
            plan,
            [
                (
                    schematic,
                    ArtifactKind::Schematic,
                    "KiCad 8+ 反向原理图交接文件",
                ),
                (manifest, ArtifactKind::Report, "KiCad 反向导入清单"),
                (
                    script,
                    ArtifactKind::Script,
                    "用于 KiCad 原理图生成的 PowerShell 自动化入口",
                ),
            ],
        )
        .await
    }

    async fn write_verification_analysis(
        &self,
        plan: &SimulationPlan,
        summaries: &mut Vec<SimulationSummary>,
    ) -> Result<()> {
        if summaries.is_empty() {
            return Ok(());
        }

        tokio::fs::create_dir_all(plan.project_root.join("reports")).await?;
        let waveform = self.waveform_digest(plan).await?;
        let overall_passed = summaries.iter().all(|summary| summary.passed);
        let passed_tools = summaries
            .iter()
            .filter(|summary| summary.passed)
            .map(|summary| summary.tool.clone())
            .collect::<Vec<_>>();
        let failing_tools = summaries
            .iter()
            .filter(|summary| !summary.passed)
            .map(|summary| summary.tool.clone())
            .collect::<Vec<_>>();

        let mut artifact_paths = summaries
            .iter()
            .flat_map(|summary| {
                summary
                    .artifacts
                    .iter()
                    .map(|artifact| artifact.path.clone())
            })
            .collect::<BTreeSet<_>>();
        let json_path = PathBuf::from("reports").join("verification_analysis.json");
        let md_path = PathBuf::from("reports").join("verification_analysis.md");
        artifact_paths.insert(normalize_arg(&json_path));
        artifact_paths.insert(normalize_arg(&md_path));

        let mut findings = summaries
            .iter()
            .map(|summary| {
                if summary.passed {
                    format!("{} 通过，退出码为 {:?}。", summary.tool, summary.exit_code)
                } else {
                    format!(
                        "{} 失败，退出码为 {:?}：{}",
                        summary.tool,
                        summary.exit_code,
                        first_non_empty_line(&summary.stderr, &summary.stdout)
                    )
                }
            })
            .collect::<Vec<_>>();

        if let Some(waveform) = &waveform {
            findings.push(format!(
                "波形 {} 从 {} 捕获了 {} 个信号。",
                waveform.path, waveform.signal_count, waveform.source
            ));
        } else if plan.generate_waveform {
            findings.push("仿真后未找到 VCD 波形产物。".to_string());
        }

        let next_actions = if overall_passed {
            vec![
                "在导入 PCB 之前，请先检查生成的 Yosys 网表和 KiCad 原理图交接文件。".to_string(),
                "当需要更深入的波形和日志推理时，请将 structured_prompt 投喂给分析模型。"
                    .to_string(),
            ]
        } else {
            vec![
                "先检查第一个失败工具的日志，并重新生成或修复受影响的 RTL/testbench。".to_string(),
                "修复后重新运行仿真，让波形摘要和网表产物反映修正后的设计。".to_string(),
            ]
        };

        let mut analysis = VerificationAnalysis {
            overall_passed,
            passed_tools,
            failing_tools,
            artifact_paths: artifact_paths.iter().cloned().collect(),
            waveform,
            findings,
            next_actions,
            structured_prompt: String::new(),
        };
        analysis.structured_prompt = build_structured_prompt(&analysis, summaries);

        tokio::fs::write(
            plan.project_root.join(&json_path),
            serde_json::to_vec_pretty(&analysis)?,
        )
        .await?;
        tokio::fs::write(
            plan.project_root.join(&md_path),
            analysis_markdown(&analysis).as_bytes(),
        )
        .await?;

        let report_artifacts = existing_artifacts(
            plan,
            [
                (
                    json_path,
                    ArtifactKind::Report,
                    "Structured verification analysis JSON",
                ),
                (
                    md_path,
                    ArtifactKind::Report,
                    "Human-readable verification analysis report",
                ),
            ],
        )
        .await?;

        if let Some(last) = summaries.last_mut() {
            last.analysis = Some(analysis);
            last.artifacts.extend(report_artifacts);
        }

        Ok(())
    }

    async fn run_command(&self, spec: CommandSpec) -> Result<ToolOutput> {
        tokio::fs::create_dir_all(&spec.cwd).await?;

        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .current_dir(&spec.cwd)
            .kill_on_drop(true)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn {}", spec.program))?;

        let timeout_duration = Duration::from_secs(spec.timeout_secs);
        let output = match timeout(timeout_duration, child.wait_with_output()).await {
            Ok(result) => result?,
            Err(_) => {
                return Ok(ToolOutput {
                    spec,
                    exit_code: None,
                    stdout: String::new(),
                    stderr: format!("command timed out after {}s", timeout_duration.as_secs()),
                    timed_out: true,
                });
            }
        };

        Ok(ToolOutput {
            spec,
            exit_code: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).to_string(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
            timed_out: false,
        })
    }
}

impl ToolOutput {
    fn passed(&self) -> bool {
        self.exit_code == Some(0) && !self.timed_out
    }

    fn into_summary(self) -> SimulationSummary {
        SimulationSummary {
            passed: self.passed(),
            tool: format!("{:?}", self.spec.tool).to_lowercase(),
            command: format!("{} {}", self.spec.program, self.spec.args.join(" ")),
            exit_code: self.exit_code,
            stdout: self.stdout,
            stderr: self.stderr,
            artifacts: Vec::new(),
            analysis: None,
        }
    }
}

async fn existing_artifacts<const N: usize>(
    plan: &SimulationPlan,
    artifacts: [(PathBuf, ArtifactKind, &'static str); N],
) -> Result<Vec<RunArtifact>> {
    let mut found = Vec::new();
    for (path, kind, description) in artifacts {
        if tokio::fs::metadata(plan.project_root.join(&path))
            .await
            .is_ok()
        {
            found.push(RunArtifact {
                path: normalize_arg(&path),
                kind,
                description: description.to_string(),
            });
        }
    }
    Ok(found)
}

async fn find_relative_files(
    root: &Path,
    roots: &[PathBuf],
    extensions: &[&str],
) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = Vec::new();

    for relative in roots {
        let full = root.join(relative);
        if tokio::fs::metadata(&full).await.is_ok() {
            stack.push(full);
        }
    }

    while let Some(dir_path) = stack.pop() {
        let mut dir = tokio::fs::read_dir(&dir_path).await?;
        while let Some(entry) = dir.next_entry().await? {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if has_extension(&path, extensions) {
                if let Ok(relative) = path.strip_prefix(root) {
                    files.push(relative.to_path_buf());
                }
            }
        }
    }

    files.sort();
    Ok(files)
}

fn has_extension(path: &Path, extensions: &[&str]) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extensions
                .iter()
                .any(|expected| extension.eq_ignore_ascii_case(expected))
        })
}

#[derive(Debug, Clone)]
struct GateDiagram {
    top: String,
    inputs: Vec<GatePort>,
    outputs: Vec<GatePort>,
    inouts: Vec<GatePort>,
    cells: Vec<GateCell>,
    edges: Vec<GateEdge>,
    net_count: usize,
}

#[derive(Debug, Clone)]
struct GatePort {
    name: String,
    bits: Vec<String>,
}

#[derive(Debug, Clone)]
struct GateCell {
    name: String,
    cell_type: String,
    inputs: Vec<GatePin>,
    outputs: Vec<GatePin>,
}

#[derive(Debug, Clone)]
struct GatePin {
    name: String,
    bits: Vec<String>,
}

#[derive(Debug, Clone)]
struct GateEdge {
    from: String,
    to: String,
    label: String,
}

fn parse_yosys_gate_diagram(requested_top: &str, json_text: &str) -> Result<GateDiagram> {
    let root: Value = serde_json::from_str(json_text)?;
    let modules = root
        .get("modules")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("Yosys JSON 中缺少 modules"))?;
    let (top, module) = modules
        .get(requested_top)
        .map(|module| (requested_top.to_string(), module))
        .or_else(|| {
            modules
                .iter()
                .next()
                .map(|(name, module)| (name.clone(), module))
        })
        .ok_or_else(|| anyhow!("Yosys JSON 中没有可绘制的模块"))?;

    let mut bit_names = BTreeMap::new();
    let mut net_count = 0;
    if let Some(netnames) = module.get("netnames").and_then(Value::as_object) {
        net_count = netnames.len();
        for (name, net) in netnames {
            let bits = value_bits(net.get("bits"));
            let clean = clean_yosys_name(name);
            for (index, bit) in bits.iter().enumerate() {
                let label = if bits.len() > 1 {
                    format!("{clean}[{index}]")
                } else {
                    clean.clone()
                };
                bit_names.entry(bit.clone()).or_insert(label);
            }
        }
    }

    let mut inputs = Vec::new();
    let mut outputs = Vec::new();
    let mut inouts = Vec::new();
    let mut input_sources = BTreeMap::new();
    let mut output_sinks = BTreeMap::new();
    if let Some(ports) = module.get("ports").and_then(Value::as_object) {
        for (name, port) in ports {
            let direction = port
                .get("direction")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let port = GatePort {
                name: clean_yosys_name(name),
                bits: value_bits(port.get("bits")),
            };
            match direction {
                "input" => {
                    for bit in &port.bits {
                        input_sources.insert(bit.clone(), format!("in:{}", port.name));
                    }
                    inputs.push(port);
                }
                "output" => {
                    for bit in &port.bits {
                        output_sinks.insert(bit.clone(), format!("out:{}", port.name));
                    }
                    outputs.push(port);
                }
                "inout" => {
                    for bit in &port.bits {
                        input_sources.insert(bit.clone(), format!("inout:{}", port.name));
                        output_sinks.insert(bit.clone(), format!("inout:{}", port.name));
                    }
                    inouts.push(port);
                }
                _ => {}
            }
        }
    }

    let mut cells = Vec::new();
    if let Some(raw_cells) = module.get("cells").and_then(Value::as_object) {
        for (name, cell) in raw_cells {
            let cell_type = cell
                .get("type")
                .and_then(Value::as_str)
                .map(display_cell_type)
                .unwrap_or_else(|| "CELL".to_string());
            let mut inputs = Vec::new();
            let mut outputs = Vec::new();
            if let Some(connections) = cell.get("connections").and_then(Value::as_object) {
                for (pin_name, bits) in connections {
                    let pin = GatePin {
                        name: clean_yosys_name(pin_name),
                        bits: value_bits(Some(bits)),
                    };
                    if is_output_cell_pin(pin_name) {
                        outputs.push(pin);
                    } else {
                        inputs.push(pin);
                    }
                }
            }
            cells.push(GateCell {
                name: clean_yosys_name(name),
                cell_type,
                inputs,
                outputs,
            });
        }
    }

    let mut cell_output_sources = BTreeMap::new();
    for (index, cell) in cells.iter().enumerate() {
        for pin in &cell.outputs {
            for bit in &pin.bits {
                cell_output_sources.insert(bit.clone(), format!("cell:{index}"));
            }
        }
    }

    let mut edge_labels: BTreeMap<(String, String), BTreeSet<String>> = BTreeMap::new();
    for (index, cell) in cells.iter().enumerate() {
        let to = format!("cell:{index}");
        for pin in &cell.inputs {
            for bit in &pin.bits {
                let from = source_for_bit(bit, &cell_output_sources, &input_sources);
                let label = format!("{}.{}", net_label(bit, &bit_names), pin.name);
                edge_labels
                    .entry((from, to.clone()))
                    .or_default()
                    .insert(label);
            }
        }
    }

    for (bit, sink) in &output_sinks {
        let from = source_for_bit(bit, &cell_output_sources, &input_sources);
        edge_labels
            .entry((from, sink.clone()))
            .or_default()
            .insert(net_label(bit, &bit_names));
    }

    let edges = edge_labels
        .into_iter()
        .map(|((from, to), labels)| GateEdge {
            from,
            to,
            label: collapse_labels(labels),
        })
        .collect();

    Ok(GateDiagram {
        top,
        inputs,
        outputs,
        inouts,
        cells,
        edges,
        net_count,
    })
}

fn value_bits(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|bits| bits.iter().map(value_bit).collect())
        .unwrap_or_default()
}

fn value_bit(value: &Value) -> String {
    if let Some(number) = value.as_u64() {
        number.to_string()
    } else if let Some(text) = value.as_str() {
        format!("const:{text}")
    } else {
        "unknown".to_string()
    }
}

fn source_for_bit(
    bit: &str,
    cell_output_sources: &BTreeMap<String, String>,
    input_sources: &BTreeMap<String, String>,
) -> String {
    cell_output_sources
        .get(bit)
        .or_else(|| input_sources.get(bit))
        .cloned()
        .unwrap_or_else(|| {
            if bit.starts_with("const:") {
                bit.to_string()
            } else {
                format!("net:{bit}")
            }
        })
}

fn net_label(bit: &str, bit_names: &BTreeMap<String, String>) -> String {
    bit_names
        .get(bit)
        .cloned()
        .unwrap_or_else(|| bit.trim_start_matches("const:").to_string())
}

fn collapse_labels(labels: BTreeSet<String>) -> String {
    let total = labels.len();
    let mut values = labels.into_iter().take(3).collect::<Vec<_>>();
    if total > values.len() {
        values.push(format!("+{} 条", total - values.len()));
    }
    values.join(", ")
}

fn is_output_cell_pin(pin_name: &str) -> bool {
    let pin = clean_yosys_name(pin_name).to_ascii_uppercase();
    matches!(pin.as_str(), "Y" | "Q" | "F" | "CO" | "COUT")
}

fn display_cell_type(value: &str) -> String {
    let clean = value.trim_matches('$').trim_matches('_');
    let clean = clean.strip_prefix('_').unwrap_or(clean);
    let clean = clean.strip_suffix('_').unwrap_or(clean);
    if clean.is_empty() {
        "CELL".to_string()
    } else {
        clean.to_ascii_uppercase()
    }
}

fn clean_yosys_name(value: &str) -> String {
    value
        .trim_start_matches('\\')
        .replace('$', "_")
        .replace(' ', "_")
}

fn gate_diagram_dot(diagram: &GateDiagram) -> String {
    let mut lines = vec![
        format!("digraph {} {{", dot_quote(&diagram.top)),
        "  rankdir=LR;".to_string(),
        "  graph [fontname=\"Arial\", bgcolor=\"white\"];".to_string(),
        "  node [fontname=\"Arial\", fontsize=10, margin=\"0.08,0.04\"];".to_string(),
        "  edge [fontname=\"Arial\", fontsize=8, color=\"#587083\"];".to_string(),
    ];

    for port in &diagram.inputs {
        lines.push(format!(
            "  {} [label={}, shape=oval, style=filled, fillcolor=\"#e8f4ff\"];",
            dot_quote(&format!("in:{}", port.name)),
            dot_quote(&format!("{}\\n{} 位", port.name, port.bits.len()))
        ));
    }
    for port in &diagram.inouts {
        lines.push(format!(
            "  {} [label={}, shape=oval, style=filled, fillcolor=\"#fff5d6\"];",
            dot_quote(&format!("inout:{}", port.name)),
            dot_quote(&format!("{}\\ninout {} 位", port.name, port.bits.len()))
        ));
    }
    for (index, cell) in diagram.cells.iter().enumerate() {
        lines.push(format!(
            "  {} [label={}, shape=box, style=\"rounded,filled\", fillcolor=\"#f6f8fa\"];",
            dot_quote(&format!("cell:{index}")),
            dot_quote(&format!("{}\\n{}", cell.cell_type, cell.name))
        ));
    }
    for port in &diagram.outputs {
        lines.push(format!(
            "  {} [label={}, shape=oval, style=filled, fillcolor=\"#eaf8ef\"];",
            dot_quote(&format!("out:{}", port.name)),
            dot_quote(&format!("{}\\n{} 位", port.name, port.bits.len()))
        ));
    }
    for node in virtual_nodes(diagram) {
        lines.push(format!(
            "  {} [label={}, shape=diamond, style=filled, fillcolor=\"#f2f0ff\"];",
            dot_quote(&node),
            dot_quote(virtual_node_label(&node).as_str())
        ));
    }
    for edge in &diagram.edges {
        lines.push(format!(
            "  {} -> {} [label={}];",
            dot_quote(&edge.from),
            dot_quote(&edge.to),
            dot_quote(&edge.label)
        ));
    }
    lines.push("}".to_string());
    lines.join("\n")
}

fn gate_diagram_svg(diagram: &GateDiagram) -> String {
    let rows_per_cell_col = 12usize;
    let cell_cols =
        ((diagram.cells.len().max(1) + rows_per_cell_col - 1) / rows_per_cell_col).max(1);
    let visual_rows = diagram
        .inputs
        .len()
        .max(diagram.outputs.len())
        .max(diagram.inouts.len())
        .max(diagram.cells.len().min(rows_per_cell_col))
        .max(1);
    let width = 520 + (cell_cols as i32 * 220);
    let height = 180 + (visual_rows as i32 * 72);
    let input_x = 70;
    let inout_x = 70;
    let first_cell_x = 260;
    let output_x = width - 180;

    let mut positions = BTreeMap::new();
    let mut body = String::new();
    body.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}" role="img" aria-label="{title}">
  <defs>
    <filter id="shadow" x="-10%" y="-20%" width="120%" height="140%">
      <feDropShadow dx="0" dy="2" stdDeviation="2" flood-color="#17202a" flood-opacity="0.14"/>
    </filter>
  </defs>
  <rect width="100%" height="100%" fill="#ffffff"/>
  <text x="32" y="34" font-family="Arial, sans-serif" font-size="20" font-weight="700" fill="#17202a">{title}</text>
  <text x="32" y="58" font-family="Arial, sans-serif" font-size="12" fill="#627386">输入 {inputs} 个，输出 {outputs} 个，双向 {inouts} 个，门/单元 {cells} 个，网名 {nets} 个</text>
"##,
        title = escape_xml(&format!("{} 门级图", diagram.top)),
        inputs = diagram.inputs.len(),
        outputs = diagram.outputs.len(),
        inouts = diagram.inouts.len(),
        cells = diagram.cells.len(),
        nets = diagram.net_count
    ));

    for (index, port) in diagram.inputs.iter().enumerate() {
        let y = 95 + (index as i32 * 72);
        let key = format!("in:{}", port.name);
        positions.insert(key, (input_x + 130, y + 24));
        body.push_str(&svg_node(
            input_x,
            y,
            130,
            48,
            "#e8f4ff",
            "#5b8db8",
            &port.name,
            &format!("输入 {} 位", port.bits.len()),
        ));
    }

    for (index, port) in diagram.inouts.iter().enumerate() {
        let y = 95 + (index as i32 * 72);
        let key = format!("inout:{}", port.name);
        positions.insert(key, (inout_x + 130, y + 24));
        body.push_str(&svg_node(
            inout_x,
            y,
            130,
            48,
            "#fff5d6",
            "#b48922",
            &port.name,
            &format!("双向 {} 位", port.bits.len()),
        ));
    }

    for (index, cell) in diagram.cells.iter().enumerate() {
        let col = index / rows_per_cell_col;
        let row = index % rows_per_cell_col;
        let x = first_cell_x + (col as i32 * 220);
        let y = 95 + (row as i32 * 72);
        positions.insert(format!("cell:{index}"), (x + 72, y + 24));
        body.push_str(&svg_node(
            x,
            y,
            144,
            48,
            "#f6f8fa",
            "#8a98a8",
            &cell.cell_type,
            &truncate_chars(&cell.name, 24),
        ));
    }

    for (index, port) in diagram.outputs.iter().enumerate() {
        let y = 95 + (index as i32 * 72);
        let key = format!("out:{}", port.name);
        positions.insert(key, (output_x, y + 24));
        body.push_str(&svg_node(
            output_x,
            y,
            130,
            48,
            "#eaf8ef",
            "#5c9a6f",
            &port.name,
            &format!("输出 {} 位", port.bits.len()),
        ));
    }

    for (index, node) in virtual_nodes(diagram).into_iter().enumerate() {
        let x = 190;
        let y = height - 74 - ((index as i32 % 4) * 34);
        positions.insert(node.clone(), (x + 46, y + 15));
        body.push_str(&svg_virtual_node(x, y, &virtual_node_label(&node)));
    }

    let mut edges = String::new();
    for (index, edge) in diagram.edges.iter().enumerate() {
        let Some(&(x1, y1)) = positions.get(&edge.from) else {
            continue;
        };
        let Some(&(x2, y2)) = positions.get(&edge.to) else {
            continue;
        };
        let offset = ((index % 5) as i32 - 2) * 4;
        let mid_x = (x1 + x2) / 2;
        let mid_y = (y1 + y2) / 2 + offset;
        edges.push_str(&format!(
            r##"  <path d="M {x1} {y1} C {c1} {y1}, {c2} {y2}, {x2} {y2}" fill="none" stroke="#587083" stroke-width="1.2" opacity="0.74"/>
  <text x="{mid_x}" y="{mid_y}" font-family="Arial, sans-serif" font-size="10" text-anchor="middle" fill="#425466" paint-order="stroke" stroke="#ffffff" stroke-width="3">{label}</text>
"##,
            c1 = x1 + 70,
            c2 = x2 - 70,
            label = escape_xml(&truncate_chars(&edge.label, 34))
        ));
    }

    format!("{body}{edges}{}</svg>\n", "")
}

fn gate_diagram_markdown(diagram: &GateDiagram, svg: &Path, dot: &Path) -> String {
    format!(
        "# 门级图表\n\n- 顶层模块：{}\n- 输入端口：{}\n- 输出端口：{}\n- 双向端口：{}\n- 门/单元数量：{}\n- 网名数量：{}\n- SVG 图：{}\n- DOT 图：{}\n\n该图由 Yosys JSON 网表直接转换生成，不依赖 Graphviz。\n",
        diagram.top,
        diagram.inputs.len(),
        diagram.outputs.len(),
        diagram.inouts.len(),
        diagram.cells.len(),
        diagram.net_count,
        normalize_arg(svg),
        normalize_arg(dot)
    )
}

fn svg_node(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    fill: &str,
    stroke: &str,
    title: &str,
    subtitle: &str,
) -> String {
    format!(
        r##"  <g filter="url(#shadow)">
    <rect x="{x}" y="{y}" width="{width}" height="{height}" rx="6" fill="{fill}" stroke="{stroke}"/>
    <text x="{tx}" y="{ty}" font-family="Arial, sans-serif" font-size="13" font-weight="700" text-anchor="middle" fill="#17202a">{title}</text>
    <text x="{tx}" y="{sy}" font-family="Arial, sans-serif" font-size="10" text-anchor="middle" fill="#627386">{subtitle}</text>
  </g>
"##,
        tx = x + width / 2,
        ty = y + 20,
        sy = y + 36,
        title = escape_xml(title),
        subtitle = escape_xml(subtitle)
    )
}

fn svg_virtual_node(x: i32, y: i32, label: &str) -> String {
    format!(
        r##"  <g>
    <rect x="{x}" y="{y}" width="92" height="30" rx="15" fill="#f2f0ff" stroke="#8b7bd8"/>
    <text x="{tx}" y="{ty}" font-family="Arial, sans-serif" font-size="10" text-anchor="middle" fill="#5142a1">{label}</text>
  </g>
"##,
        tx = x + 46,
        ty = y + 19,
        label = escape_xml(label)
    )
}

fn virtual_nodes(diagram: &GateDiagram) -> Vec<String> {
    let real_nodes = diagram
        .inputs
        .iter()
        .map(|port| format!("in:{}", port.name))
        .chain(
            diagram
                .outputs
                .iter()
                .map(|port| format!("out:{}", port.name)),
        )
        .chain(
            diagram
                .inouts
                .iter()
                .map(|port| format!("inout:{}", port.name)),
        )
        .chain((0..diagram.cells.len()).map(|index| format!("cell:{index}")))
        .collect::<BTreeSet<_>>();

    diagram
        .edges
        .iter()
        .flat_map(|edge| [edge.from.clone(), edge.to.clone()])
        .filter(|node| !real_nodes.contains(node))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn virtual_node_label(node: &str) -> String {
    node.strip_prefix("const:")
        .map(|value| format!("常量 {value}"))
        .or_else(|| {
            node.strip_prefix("net:")
                .map(|value| format!("net {value}"))
        })
        .unwrap_or_else(|| node.to_string())
}

fn dot_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn escape_xml(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn parse_vcd_digest(path: &Path, text: &str) -> WaveformDigest {
    let mut timescale = None;
    let mut in_timescale = false;
    let mut signals = Vec::new();
    let mut transitions_sample = Vec::new();

    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("$timescale") {
            if let Some(value) = line
                .trim_start_matches("$timescale")
                .trim_end_matches("$end")
                .trim()
                .strip_prefix(' ')
            {
                timescale = Some(value.trim().to_string());
            } else if line.ends_with("$end") {
                let value = line
                    .trim_start_matches("$timescale")
                    .trim_end_matches("$end")
                    .trim();
                if !value.is_empty() {
                    timescale = Some(value.to_string());
                }
            } else {
                in_timescale = true;
            }
            continue;
        }

        if in_timescale {
            let value = line.trim_end_matches("$end").trim();
            if !value.is_empty() {
                timescale = Some(value.to_string());
                in_timescale = false;
            }
            continue;
        }

        if line.starts_with("$var") {
            if let Some(signal) = parse_vcd_signal(line) {
                signals.push(signal);
            }
            continue;
        }

        if transitions_sample.len() < 40 && is_vcd_transition_line(line) {
            transitions_sample.push(line.to_string());
        }
    }

    let signal_count = signals.len();
    let signal_preview = signals
        .iter()
        .take(16)
        .map(|signal| signal.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let ocr_text = format!(
        "波形 OCR 来源=direct_vcd_text path={} 时间尺度={} 信号数={} 信号=[{}] 采样跳变={}",
        normalize_arg(path),
        timescale.as_deref().unwrap_or("unknown"),
        signal_count,
        signal_preview,
        transitions_sample.join(" | ")
    );

    WaveformDigest {
        path: normalize_arg(path),
        source: "direct_vcd_text_extraction".to_string(),
        timescale,
        signal_count,
        signals: signals.into_iter().take(64).collect(),
        transitions_sample,
        ocr_text,
    }
}

fn parse_vcd_signal(line: &str) -> Option<WaveformSignal> {
    let parts = line.split_whitespace().collect::<Vec<_>>();
    if parts.len() < 5 {
        return None;
    }

    let width = parts.get(2).and_then(|value| value.parse::<u32>().ok());
    let mut name = parts[4].to_string();
    if parts.len() > 6 && parts[5] != "$end" {
        name.push(' ');
        name.push_str(parts[5]);
    }

    Some(WaveformSignal { name, width })
}

fn is_vcd_transition_line(line: &str) -> bool {
    line.starts_with('#')
        || line.starts_with('b')
        || line
            .chars()
            .next()
            .is_some_and(|ch| matches!(ch, '0' | '1' | 'x' | 'X' | 'z' | 'Z'))
}

fn kicad_schematic(top: &str, gate_verilog: &Path, netlist_json: &Path) -> String {
    let generator = "chip-agent";
    let schematic_uuid = uuid::Uuid::new_v4();
    let text_uuid_1 = uuid::Uuid::new_v4();
    let text_uuid_2 = uuid::Uuid::new_v4();
    let text_uuid_3 = uuid::Uuid::new_v4();

    format!(
        r#"(kicad_sch
  (version 20230121)
  (generator "{generator}")
  (uuid "{schematic_uuid}")
  (paper "A4")
  (title_block
    (title "{top} gate netlist hand-off")
    (company "chip-agent")
    (comment 1 "由 Yosys 综合产物生成，用于 KiCad 8+ 反向原理图自动化")
  )
  (text "顶层模块：{top}" (at 20 25 0)
    (effects (font (size 1.5 1.5)))
    (uuid "{text_uuid_1}")
  )
  (text "门级 Verilog：{gate_verilog}" (at 20 35 0)
    (effects (font (size 1.2 1.2)))
    (uuid "{text_uuid_2}")
  )
  (text "Yosys JSON 网表：{netlist_json}" (at 20 43 0)
    (effects (font (size 1.2 1.2)))
    (uuid "{text_uuid_3}")
  )
)
"#,
        gate_verilog = normalize_arg(gate_verilog),
        netlist_json = normalize_arg(netlist_json)
    )
}

fn kicad_script(top: &str, schematic: &Path, manifest: &Path) -> String {
    format!(
        r#"$ErrorActionPreference = "Stop"
$ScriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$Schematic = Join-Path $ScriptRoot "{schematic_name}"
$Manifest = Join-Path $ScriptRoot "{manifest_name}"

Write-Host "KiCad 反向原理图交接，顶层模块：{top}"
Write-Host "原理图：$Schematic"
Write-Host "清单：$Manifest"

if (-not (Test-Path $Schematic)) {{
    throw "缺少生成的原理图：$Schematic"
}}

if (-not (Test-Path $Manifest)) {{
    throw "缺少 KiCad 导入清单：$Manifest"
}}

if (Get-Command kicad-cli -ErrorAction SilentlyContinue) {{
    Write-Host "已检测到 kicad-cli。请在 KiCad 8+ 中打开生成的原理图继续布局和导入审查。"
}} else {{
    Write-Warning "在 PATH 中未找到 kicad-cli。请安装 KiCad 8+，或手动打开 .kicad_sch 文件。"
}}
"#,
        top = top,
        schematic_name = schematic
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("generated.kicad_sch"),
        manifest_name = manifest
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("kicad_import_manifest.json")
    )
}

fn build_structured_prompt(
    analysis: &VerificationAnalysis,
    summaries: &[SimulationSummary],
) -> String {
    let tool_logs = summaries
        .iter()
        .map(|summary| {
            format!(
                "tool={tool}\npassed={passed}\nexit_code={exit:?}\nstdout={stdout}\nstderr={stderr}",
                tool = summary.tool,
                passed = summary.passed,
                exit = summary.exit_code,
                stdout = truncate_chars(&summary.stdout, 1800),
                stderr = truncate_chars(&summary.stderr, 1800)
            )
        })
        .collect::<Vec<_>>()
        .join("\n---\n");

    let waveform_text = analysis
        .waveform
        .as_ref()
        .map(|waveform| waveform.ocr_text.as_str())
        .unwrap_or("暂无波形 OCR 文本。");

    format!(
        r#"你是一名数字设计验证分析员。

请仅返回严格 JSON，键名为：verdict、root_cause、evidence、waveform_observations、recommended_next_steps。

整体是否通过：{overall_passed}
通过的工具：{passed_tools}
失败的工具：{failing_tools}
产物：{artifacts}

波形 OCR/文本：
{waveform_text}

工具日志：
{tool_logs}
"#,
        overall_passed = analysis.overall_passed,
        passed_tools = analysis.passed_tools.join(", "),
        failing_tools = analysis.failing_tools.join(", "),
        artifacts = analysis.artifact_paths.join(", "),
    )
}

fn analysis_markdown(analysis: &VerificationAnalysis) -> String {
    let mut lines = vec![
        "# 验证分析".to_string(),
        String::new(),
        format!("- 整体是否通过：{}", analysis.overall_passed),
        format!("- 通过的工具：{}", analysis.passed_tools.join(", ")),
        format!("- 失败的工具：{}", analysis.failing_tools.join(", ")),
        String::new(),
        "## 发现".to_string(),
    ];

    lines.extend(
        analysis
            .findings
            .iter()
            .map(|finding| format!("- {finding}")),
    );
    lines.push(String::new());
    lines.push("## 下一步".to_string());
    lines.extend(
        analysis
            .next_actions
            .iter()
            .map(|action| format!("- {action}")),
    );
    lines.push(String::new());
    lines.push("## 结构化提示词".to_string());
    lines.push("```text".to_string());
    lines.push(analysis.structured_prompt.clone());
    lines.push("```".to_string());
    lines.join("\n")
}

fn first_non_empty_line(primary: &str, fallback: &str) -> String {
    primary
        .lines()
        .chain(fallback.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(truncate_error_line)
        .unwrap_or_else(|| "未捕获到诊断行".to_string())
}

fn truncate_error_line(line: &str) -> String {
    truncate_chars(line, 240)
}

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut iter = value.chars();
    let truncated = iter.by_ref().take(max_chars).collect::<String>();
    if iter.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

async fn command_exists(program: &str) -> bool {
    let probe = if cfg!(windows) { "where.exe" } else { "which" };
    Command::new(probe)
        .arg(program)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await
        .is_ok_and(|status| status.success())
}

async fn resolve_program(candidates: &[&str]) -> Result<String> {
    for candidate in candidates {
        if command_exists(candidate).await {
            return Ok((*candidate).to_string());
        }
    }

    Err(anyhow!("在 PATH 中未找到以下任何程序：{candidates:?}"))
}

fn validate_paths(plan: &SimulationPlan) -> Result<()> {
    if plan.rtl_files.is_empty() {
        return Err(anyhow!("至少需要一个 RTL 文件"));
    }

    for path in plan.rtl_files.iter().chain(plan.testbench_files.iter()) {
        if path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err(anyhow!("路径不得包含父目录组件：{path:?}"));
        }
    }

    Ok(())
}

fn normalize_arg(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn safe_identifier(value: &str, fallback: &str) -> String {
    let safe = value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
        .collect::<String>();

    if safe.is_empty() {
        fallback.to_string()
    } else {
        safe
    }
}
