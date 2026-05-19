use anyhow::Result;
use async_trait::async_trait;
use domain::{RepairSuggestion, SimulationSummary};

#[async_trait]
pub trait RepairAgent: Send + Sync + 'static {
    async fn suggest(&self, summaries: &[SimulationSummary]) -> Result<RepairSuggestion>;
}

#[derive(Debug, Default)]
pub struct HeuristicRepairAgent;

#[async_trait]
impl RepairAgent for HeuristicRepairAgent {
    async fn suggest(&self, summaries: &[SimulationSummary]) -> Result<RepairSuggestion> {
        let failed = summaries.iter().find(|summary| !summary.passed);
        let reason = failed
            .map(classify_failure)
            .unwrap_or_else(|| "未提供失败的仿真摘要。".to_string());

        Ok(RepairSuggestion {
            reason,
            artifacts: Vec::new(),
            should_retry: false,
        })
    }
}

fn classify_failure(summary: &SimulationSummary) -> String {
    let log = format!("{}\n{}", summary.stdout, summary.stderr).to_ascii_lowercase();

    if log.contains("syntax error") {
        "仿真器报告语法错误。请围绕报错行重新生成受影响的 RTL/testbench。".to_string()
    } else if log.contains("width") {
        "仿真器报告位宽不匹配。请检查信号声明和类型转换。".to_string()
    } else if log.contains("unsupported") {
        "仿真器报告不支持的语法。请优先使用 Verilator/Icarus 支持的可综合 SystemVerilog。"
            .to_string()
    } else if summary.exit_code.is_none() {
        "仿真器没有返回正常退出码，可能是超时或工具缺失。".to_string()
    } else {
        "仿真失败。请检查 stdout/stderr，并在缩小失败断言范围后重新运行。".to_string()
    }
}
