use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::Path;
use std::process;
use chrono::Utc;

enum Language {
    Rust,
    JavaScript,
    Unsupported,
}

impl Language {
    fn as_str(&self) -> &'static str {
        match self {
            Language::Rust => "Rust",
            Language::JavaScript => "JavaScript",
            Language::Unsupported => "Unsupported",
        }
    }
}

struct AnalysisResult {
    source_file: String,
    language: String,
    analyzed_at: String,
    total_lines: usize,
    non_empty_lines: usize,
    import_lines: usize,
    function_definitions: usize,
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() == 2 {
        let cmd_or_file = &args[1];
        if cmd_or_file == "analyze" || cmd_or_file == "compare" {
            print_usage(&args[0]);
            process::exit(1);
        }

        // Legacy behavior: analyze and print to stdout
        match analyze_file(cmd_or_file) {
            Ok(result) => {
                println!("Total lines: {}", result.total_lines);
                println!("Non-empty lines: {}", result.non_empty_lines);
                println!("Language: {}", result.language);
                if result.language == "Unsupported" {
                    println!("Import and function detection are not supported for this file type.");
                } else {
                    println!("Import lines: {}", result.import_lines);
                    println!("Function definitions: {}", result.function_definitions);
                }
            }
            Err(e) => {
                eprintln!("Error: Could not read file '{}'. Details: {}", cmd_or_file, e);
                process::exit(1);
            }
        }
    } else if args.len() == 4 && args[1] == "analyze" {
        let source_file = &args[2];
        let log_file = &args[3];

        match analyze_file(source_file) {
            Ok(result) => {
                if let Err(e) = save_analysis_result(&result, log_file) {
                    eprintln!("Error: Could not save analysis result to '{}'. Details: {}", log_file, e);
                    process::exit(1);
                }
                println!("Analysis log saved to: {}", log_file);
            }
            Err(e) => {
                eprintln!("Error: Could not read file '{}'. Details: {}", source_file, e);
                process::exit(1);
            }
        }
    } else if args.len() == 4 && args[1] == "compare" {
        let first_log = &args[2];
        let second_log = &args[3];

        if let Err(e) = compare_results(first_log, second_log) {
            eprintln!("Error during comparison. Details: {}", e);
            process::exit(1);
        }
    } else {
        let program_name = args.get(0).map(|s| s.as_str()).unwrap_or("code-analyzer");
        print_usage(program_name);
        process::exit(1);
    }
}

fn print_usage(program_name: &str) {
    eprintln!("Usage:");
    eprintln!("  {} <source-file>", program_name);
    eprintln!("  {} analyze <source-file> <log-file>", program_name);
    eprintln!("  {} compare <first-log-file> <second-log-file>", program_name);
}

/// Analyzes a source file and returns an `AnalysisResult`
fn analyze_file(filename: &str) -> io::Result<AnalysisResult> {
    let path = Path::new(filename);
    let extension = path.extension().and_then(|ext| ext.to_str());
    let language = match extension {
        Some("rs") => Language::Rust,
        Some("js") | Some("mjs") | Some("cjs") => Language::JavaScript,
        _ => Language::Unsupported,
    };

    let file = File::open(filename)?;
    let reader = BufReader::new(file);

    let mut total_lines = 0;
    let mut non_empty_lines = 0;
    let mut import_lines = 0;
    let mut function_definitions = 0;

    for line in reader.lines() {
        let line = line?;
        total_lines += 1;

        if !line.trim().is_empty() {
            non_empty_lines += 1;
        }

        let trimmed_start = line.trim_start();
        match language {
            Language::Rust => {
                if trimmed_start.starts_with("use ") || trimmed_start.starts_with("extern crate ") {
                    import_lines += 1;
                }
                if is_rust_function_definition(&line) {
                    function_definitions += 1;
                }
            }
            Language::JavaScript => {
                if trimmed_start.starts_with("import ") || trimmed_start.contains("require(") {
                    import_lines += 1;
                }
                if is_javascript_function_definition(&line) {
                    function_definitions += 1;
                }
            }
            Language::Unsupported => {}
        }
    }

    let analyzed_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    Ok(AnalysisResult {
        source_file: filename.to_string(),
        language: language.as_str().to_string(),
        analyzed_at,
        total_lines,
        non_empty_lines,
        import_lines,
        function_definitions,
    })
}

/// Saves an `AnalysisResult` to a structured key-value log file.
fn save_analysis_result(result: &AnalysisResult, log_filename: &str) -> io::Result<()> {
    let mut file = File::create(log_filename)?;
    writeln!(file, "source_file={}", result.source_file)?;
    writeln!(file, "language={}", result.language)?;
    writeln!(file, "analyzed_at={}", result.analyzed_at)?;
    writeln!(file, "total_lines={}", result.total_lines)?;
    writeln!(file, "non_empty_lines={}", result.non_empty_lines)?;
    writeln!(file, "import_lines={}", result.import_lines)?;
    writeln!(file, "function_definitions={}", result.function_definitions)?;
    Ok(())
}

/// Reads and parses an `AnalysisResult` from a structured log file.
fn parse_analysis_result(log_filename: &str) -> io::Result<AnalysisResult> {
    let file = File::open(log_filename).map_err(|e| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("Could not open log file '{}'. Details: {}", log_filename, e),
        )
    })?;
    let reader = BufReader::new(file);

    let mut source_file = None;
    let mut language = None;
    let mut analyzed_at = None;
    let mut total_lines = None;
    let mut non_empty_lines = None;
    let mut import_lines = None;
    let mut function_definitions = None;

    for (line_idx, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let parts: Vec<&str> = line.splitn(2, '=').collect();
        if parts.len() != 2 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Malformed entry in '{}' at line {}: {}", log_filename, line_idx + 1, line),
            ));
        }
        let key = parts[0].trim();
        let val = parts[1].trim().to_string();

        match key {
            "source_file" => source_file = Some(val),
            "language" => language = Some(val),
            "analyzed_at" => analyzed_at = Some(val),
            "total_lines" => {
                total_lines = Some(val.parse::<usize>().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Invalid number for total_lines in '{}': {}", log_filename, val),
                    )
                })?);
            }
            "non_empty_lines" => {
                non_empty_lines = Some(val.parse::<usize>().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Invalid number for non_empty_lines in '{}': {}", log_filename, val),
                    )
                })?);
            }
            "import_lines" => {
                import_lines = Some(val.parse::<usize>().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Invalid number for import_lines in '{}': {}", log_filename, val),
                    )
                })?);
            }
            "function_definitions" => {
                function_definitions = Some(val.parse::<usize>().map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "Invalid number for function_definitions in '{}': {}",
                            log_filename, val
                        ),
                    )
                })?);
            }
            _ => {}
        }
    }

    Ok(AnalysisResult {
        source_file: source_file.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Missing required field 'source_file' in '{}'", log_filename),
            )
        })?,
        language: language.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Missing required field 'language' in '{}'", log_filename),
            )
        })?,
        analyzed_at: analyzed_at.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Missing required field 'analyzed_at' in '{}'", log_filename),
            )
        })?,
        total_lines: total_lines.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Missing required field 'total_lines' in '{}'", log_filename),
            )
        })?,
        non_empty_lines: non_empty_lines.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Missing required field 'non_empty_lines' in '{}'", log_filename),
            )
        })?,
        import_lines: import_lines.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("Missing required field 'import_lines' in '{}'", log_filename),
            )
        })?,
        function_definitions: function_definitions.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Missing required field 'function_definitions' in '{}'",
                    log_filename
                ),
            )
        })?,
    })
}

/// Compares two log files and prints the differences.
fn compare_results(first_log: &str, second_log: &str) -> io::Result<()> {
    let old = parse_analysis_result(first_log)?;
    let new = parse_analysis_result(second_log)?;

    println!("Comparison: {} → {}", first_log, second_log);
    println!();
    println!("Source file:          {} → {}", old.source_file, new.source_file);
    println!("Language:             {} → {}", old.language, new.language);
    println!("Analyzed at:          {} → {}", old.analyzed_at, new.analyzed_at);
    println!();

    print_comparison_metric("Total lines:", old.total_lines, new.total_lines);
    print_comparison_metric("Non-empty lines:", old.non_empty_lines, new.non_empty_lines);
    print_comparison_metric("Import lines:", old.import_lines, new.import_lines);
    print_comparison_metric("Function definitions:", old.function_definitions, new.function_definitions);

    Ok(())
}

fn print_comparison_metric(label: &str, old_val: usize, new_val: usize) {
    let diff = (new_val as isize) - (old_val as isize);
    let diff_sign = if diff > 0 { "+" } else { "" };
    
    let status = if diff > 0 {
        "increased"
    } else if diff < 0 {
        "decreased"
    } else {
        "unchanged"
    };

    println!(
        "{:<22} {:>3} → {:<4} ({}{}, {})",
        label, old_val, new_val, diff_sign, diff, status
    );
}

/// Helper to determine if a line in a Rust file represents a function definition.
fn is_rust_function_definition(line: &str) -> bool {
    line.trim_start().starts_with("fn ")
}

/// Helper to determine if a line in a JavaScript file represents a function definition.
fn is_javascript_function_definition(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("function ") || trimmed.starts_with("async function ") {
        return true;
    }
    if (trimmed.starts_with("const ") || trimmed.starts_with("let ") || trimmed.starts_with("var "))
        && trimmed.contains("=>")
    {
        return true;
    }
    false
}
