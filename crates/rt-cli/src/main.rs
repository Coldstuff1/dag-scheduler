use std::{fs, path::PathBuf, process::ExitCode};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(about = "Real-time task DAG tooling built on the `rt` MLIR dialect")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Convert a GraphViz DOT DAG into `rt` dialect MLIR.
    Import {
        /// Input `.dot` file.
        input: PathBuf,
        /// Output file (stdout if omitted).
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Re-emit a DOT DAG with WCET/period in the node labels, for rendering:
    /// `rt-cli viz in.dot | dot -Tsvg -o out.svg`.
    Viz {
        /// Input `.dot` file.
        input: PathBuf,
        /// Output file (stdout if omitted).
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Print the IRDL definition of the `rt` dialect (for `mlir-opt --irdl-file`).
    Irdl,
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

fn read_dag(input: &PathBuf) -> Result<rt_schema::Dag> {
    let src = fs::read_to_string(input).map_err(|e| format!("{}: {e}", input.display()))?;
    Ok(rt_schema::parse_dot(&src)?)
}

fn import(input: &PathBuf) -> Result<String> {
    let dag = read_dag(input)?;
    let context = rt_dialect::create_context()?;
    let module = rt_dialect::build_module(&context, &dag)?;
    Ok(module.as_operation().to_string())
}

fn write_out(path: Option<PathBuf>, text: &str) -> Result<()> {
    match path {
        Some(path) => Ok(fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?),
        None => Ok(print!("{text}")),
    }
}

fn main() -> ExitCode {
    let result = match Cli::parse().command {
        Command::Import { input, output } => import(&input).and_then(|ir| write_out(output, &ir)),
        Command::Viz { input, output } => {
            read_dag(&input).and_then(|dag| write_out(output, &rt_emit::to_dot(&dag)))
        }
        Command::Irdl => write_out(None, rt_dialect::RT_IRDL),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples").join(name)
    }

    #[test]
    fn import_pipeline_snapshot() {
        insta::assert_snapshot!(import(&example("pipeline.dot")).unwrap());
    }

    #[test]
    fn import_rejects_bad_examples() {
        let err = import(&example("two_sinks.dot")).unwrap_err().to_string();
        assert!(err.contains("exactly one sink"), "{err}");
        let err = import(&example("bad_sensor.dot")).unwrap_err().to_string();
        assert!(err.contains("sensor `imu` has incoming edges"), "{err}");
    }
}
