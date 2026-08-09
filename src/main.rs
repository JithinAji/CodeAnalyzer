use std::env;
use std::fs::File;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process;
use chrono::Utc;

use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
    Terminal,
};

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

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct FunctionResult {
    name: String,
    start_line: usize,
    end_line: usize,
    total_lines: usize,
    non_empty_lines: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct ScopeInfo {
    #[serde(rename = "type")]
    scope_type: String,
    name: String,
    file: String,
    start_line: usize,
    end_line: usize,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct AnalysisResult {
    source_file: String,
    language: String,
    analyzed_at: String,
    total_lines: usize,
    non_empty_lines: usize,
    import_lines: usize,
    #[serde(default)]
    scope: Option<ScopeInfo>,
    function_definitions: usize,
    functions: Vec<FunctionResult>,
}

#[derive(Debug, PartialEq)]
enum Command {
    Analyze {
        source_file: String,
        output_file: Option<String>,
        function_name: Option<String>,
    },
    Compare {
        log_files: Vec<String>,
        output_file: Option<String>,
        function_name: Option<String>,
    },
    Help,
    Interactive,
}

fn extract_output_flag(args: &mut Vec<String>) -> Option<String> {
    if let Some(pos) = args.iter().position(|x| x == "--output") {
        if pos + 1 < args.len() {
            let val = args.remove(pos + 1);
            args.remove(pos);
            return Some(val);
        }
    }
    None
}

fn extract_function_flag(args: &mut Vec<String>) -> Option<String> {
    if let Some(pos) = args.iter().position(|x| x == "--function") {
        if pos + 1 < args.len() {
            let val = args.remove(pos + 1);
            args.remove(pos);
            return Some(val);
        }
    }
    None
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    let mut args_vec = args.to_vec();
    let output_file = extract_output_flag(&mut args_vec);
    let function_name = extract_function_flag(&mut args_vec);

    if args_vec.len() < 2 {
        return Ok(Command::Interactive);
    }
    match args_vec[1].as_str() {
        "analyze" => {
            if args_vec.len() < 3 {
                return Err("Missing source file for 'analyze' command.".to_string());
            }
            let source_file = args_vec[2].clone();
            let final_output = if output_file.is_some() {
                output_file
            } else if args_vec.len() >= 4 {
                Some(args_vec[3].clone())
            } else {
                None
            };
            Ok(Command::Analyze {
                source_file,
                output_file: final_output,
                function_name,
            })
        }
        "compare" => {
            if args_vec.len() < 4 {
                return Err("Invalid number of arguments for 'compare' command. Require at least two input log files.".to_string());
            }
            let log_files = args_vec[2..].to_vec();
            Ok(Command::Compare {
                log_files,
                output_file,
                function_name,
            })
        }
        "compare-multi" => {
            eprintln!("Warning: 'compare-multi' is deprecated. Please use 'compare' instead.");
            if args_vec.len() < 4 {
                return Err("Invalid number of arguments for 'compare' command. Require at least two input log files.".to_string());
            }
            let log_files = args_vec[2..].to_vec();
            Ok(Command::Compare {
                log_files,
                output_file,
                function_name,
            })
        }
        "interactive" => {
            Ok(Command::Interactive)
        }
        "--help" | "-h" | "help" => Ok(Command::Help),
        cmd => Err(format!("Unknown command '{}'.", cmd)),
    }
}

fn load_log_target(path: &str, default_label: String, function_name: Option<&str>) -> io::Result<LogMetadata> {
    let current_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let resolved_path = match resolve_path(path, &current_dir) {
        Ok(p) => p,
        Err(e) => return Err(io::Error::new(io::ErrorKind::NotFound, e)),
    };
    let path_str = resolved_path.to_string_lossy().to_string();
    let lower_path = path_str.to_lowercase();
    if lower_path.ends_with(".rs") || lower_path.ends_with(".js") || lower_path.ends_with(".mjs") || lower_path.ends_with(".cjs") {
        let mut res = analyze_file(&path_str)?;
        if let Some(func_name) = function_name {
            apply_function_scope(&mut res, func_name)?;
        }
        Ok(LogMetadata {
            label: default_label,
            source: path_str,
            total_lines: res.total_lines,
            error_count: 0,
            warn_count: 0,
            info_count: 0,
            avg_latency_ms: None,
            max_latency_ms: None,
            min_latency_ms: None,
            latency_values: Vec::new(),
            is_code_analyzer_log: true,
            code_total_lines: Some(res.total_lines),
            code_non_empty_lines: Some(res.non_empty_lines),
            code_import_lines: Some(res.import_lines),
            code_function_definitions: Some(res.function_definitions),
        })
    } else {
        let content = std::fs::read_to_string(&path_str).map_err(|e| {
            io::Error::new(
                io::ErrorKind::NotFound,
                format!("Could not open log file '{}'. Details: {}", path_str, e),
            )
        })?;

        let mut parsed_res: Option<AnalysisResult> = None;
        if content.trim_start().starts_with('{') {
            if let Ok(analyze_res) = serde_json::from_str::<AnalyzeJsonResult>(&content) {
                parsed_res = Some(analyze_res.result);
            } else if let Ok(analysis_res) = serde_json::from_str::<AnalysisResult>(&content) {
                parsed_res = Some(analysis_res);
            }
        } else if content.contains("source_file=") && content.contains("language=") && content.contains("total_lines=") {
            if let Ok(analysis_res) = parse_analysis_result(&path_str) {
                parsed_res = Some(analysis_res);
            }
        }

        if let Some(mut res) = parsed_res {
            if let Some(func_name) = function_name {
                if let Some(ref scope) = res.scope {
                    if scope.name != func_name {
                        return Err(io::Error::new(
                            io::ErrorKind::NotFound,
                            format!("Log file '{}' is scoped to function '{}', but requested '{}'", path_str, scope.name, func_name),
                        ));
                    }
                } else {
                    apply_function_scope(&mut res, func_name)?;
                }
            }
            Ok(LogMetadata {
                label: default_label,
                source: path_str,
                total_lines: res.total_lines,
                error_count: 0,
                warn_count: 0,
                info_count: 0,
                avg_latency_ms: None,
                max_latency_ms: None,
                min_latency_ms: None,
                latency_values: Vec::new(),
                is_code_analyzer_log: true,
                code_total_lines: Some(res.total_lines),
                code_non_empty_lines: Some(res.non_empty_lines),
                code_import_lines: Some(res.import_lines),
                code_function_definitions: Some(res.function_definitions),
            })
        } else {
            if function_name.is_some() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("Cannot perform function-scoped analysis on non-code-analyzer log file '{}'", path_str),
                ));
            }
            Ok(parse_log_content(&content, default_label, path_str))
        }
    }
}

fn execute_compare(log_paths: &[String], output_file: Option<&str>, function_name: Option<&str>) -> io::Result<()> {
    if log_paths.len() < 2 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Invalid number of arguments for 'compare' command. Require at least two input log files.",
        ));
    }

    if let Some(func_name) = function_name {
        println!("=== Function-scoped Comparison: '{}' ===", func_name);
    }

    let mut logs = Vec::new();
    for path in log_paths {
        let label = Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Log")
            .to_string();
        let log = load_log_target(path, label, function_name)?;
        logs.push(log);
    }

    let prompt = "General comparison";
    
    println!("\n--- Key Findings Summary ---");
    print_summary(&logs, prompt);

    println!("\n--- Numeric Comparison Table ---");
    print_comparison_table(&logs, prompt);

    let active_labels: Vec<String> = logs.iter().map(|l| l.label.clone()).collect();
    let values: Vec<f64> = if logs.iter().all(|l| l.is_code_analyzer_log) {
        logs.iter().map(|l| l.code_total_lines.unwrap_or(0) as f64).collect()
    } else {
        logs.iter().map(|l| l.error_count as f64).collect()
    };
    let title = if logs.iter().all(|l| l.is_code_analyzer_log) { "Total Source Code Lines" } else { "Error Count" };
    draw_bar_chart(&active_labels, &values, title);

    if logs.len() == 2 && logs[0].is_code_analyzer_log && logs[1].is_code_analyzer_log {
        println!("\n--- Function-Level Comparisons ---");
        let _ = compare_current_with_log(&log_paths[0], &log_paths[1], function_name);
    }

    let metadata = CompareMetadata {
        command: "compare".to_string(),
        timestamp: get_fs_safe_timestamp(),
        input_files: log_paths.to_vec(),
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        prompt: Some(prompt.to_string()),
    };

    let graph_data = GraphDataJson {
        title: title.to_string(),
        labels: active_labels,
        values,
    };

    let mut table_data = Vec::new();
    table_data.push(vec!["Label".to_string()].into_iter().chain(logs.iter().map(|l| l.label.clone())).collect::<Vec<String>>());
    table_data.push(vec!["Source".to_string()].into_iter().chain(logs.iter().map(|l| l.source.clone())).collect::<Vec<String>>());
    table_data.push(vec!["Total Lines".to_string()].into_iter().chain(logs.iter().map(|l| l.total_lines.to_string())).collect::<Vec<String>>());
    table_data.push(vec!["Error Count".to_string()].into_iter().chain(logs.iter().map(|l| l.error_count.to_string())).collect::<Vec<String>>());
    table_data.push(vec!["Warn Count".to_string()].into_iter().chain(logs.iter().map(|l| l.warn_count.to_string())).collect::<Vec<String>>());

    let saved_path = save_compare_result_json(
        metadata,
        logs,
        "General comparison summary".to_string(),
        table_data,
        graph_data,
        output_file,
    )?;

    println!("\nSaved comparison result to: {}", saved_path);

    Ok(())
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let program_name = args.get(0).map(|s| s.as_str()).unwrap_or("codeanalyzer");

    match parse_args(&args) {
        Ok(Command::Analyze { source_file, output_file, function_name }) => {
            let current_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let resolved_path = match resolve_path(&source_file, &current_dir) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("{}", e);
                    process::exit(1);
                }
            };
            let path_str = resolved_path.to_string_lossy().to_string();
            match analyze_file(&path_str) {
                Ok(mut result) => {
                    if let Some(ref func_name) = function_name {
                        if let Err(e) = apply_function_scope(&mut result, func_name) {
                            eprintln!("Error: {}", e);
                            process::exit(1);
                        }
                        println!("=== Function-scoped Analysis ===");
                        if let Some(ref scope) = result.scope {
                            println!("Function name:   {}", scope.name);
                            println!("Source location: {}:{}-{}", scope.file, scope.start_line, scope.end_line);
                        }
                        println!("=================================");
                    }

                    println!("Total lines: {}", result.total_lines);
                    println!("Non-empty lines: {}", result.non_empty_lines);
                    println!("Language: {}", result.language);
                    if result.language == "Unsupported" {
                        println!("Import and function detection are not supported for this file type.");
                    } else {
                        println!("Import lines: {}", result.import_lines);
                        println!("Function definitions: {}", result.function_definitions);
                        if !result.functions.is_empty() {
                            println!("\nFunctions:");
                            for func in &result.functions {
                                println!(
                                    "  - {} (lines {}-{}, total: {}, non-empty: {})",
                                    func.name, func.start_line, func.end_line, func.total_lines, func.non_empty_lines
                                );
                            }
                        }
                    }

                    match save_analysis_result_json(&result, output_file.as_deref()) {
                        Ok(saved_path) => {
                            println!("\nSaved analysis result to: {}", saved_path);
                        }
                        Err(e) => {
                            eprintln!("Error: Could not save analysis result. Details: {}", e);
                            process::exit(1);
                        }
                    }

                    match save_analysis_result_log(&result, output_file.as_deref()) {
                        Ok(saved_path) => {
                            println!("Saved log file to: {}", saved_path);
                        }
                        Err(e) => {
                            eprintln!("Error: Could not save log file. Details: {}", e);
                            process::exit(1);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("Error: Could not read file '{}'. Details: {}", path_str, e);
                    process::exit(1);
                }
            }
        }
        Ok(Command::Compare { log_files, output_file, function_name }) => {
            let current_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
            let mut resolved_log_files = Vec::new();
            for file in log_files {
                match resolve_path(&file, &current_dir) {
                    Ok(p) => resolved_log_files.push(p.to_string_lossy().to_string()),
                    Err(e) => {
                        eprintln!("{}", e);
                        process::exit(1);
                    }
                }
            }
            if let Err(e) = execute_compare(&resolved_log_files, output_file.as_deref(), function_name.as_deref()) {
                eprintln!("Error: {}", e);
                process::exit(1);
            }
        }
        Ok(Command::Help) => {
            print_usage(program_name);
            process::exit(0);
        }
        Ok(Command::Interactive) => {
            if let Err(e) = run_tui() {
                eprintln!("TUI Error: {}", e);
                process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("Error: {}", e);
            print_usage(program_name);
            process::exit(1);
        }
    }
}

fn print_usage(program_name: &str) {
    eprintln!("Usage:");
    eprintln!("  {} analyze <source-file> [--output <path>] [--function <name>]", program_name);
    eprintln!("  {} compare <log1> <log2> [log3 ...] [--output <path>] [--function <name>]", program_name);
    eprintln!("  {} (without arguments runs interactive terminal menu)", program_name);
}

use std::path::Component;

fn normalize_path(path: &Path) -> PathBuf {
    let mut components = path.components().peekable();
    let mut ret = PathBuf::new();
    if let Some(c @ Component::Prefix(..)) = components.peek() {
        ret.push(c.as_os_str());
        components.next();
    }
    if let Some(c @ Component::RootDir) = components.peek() {
        ret.push(c.as_os_str());
        components.next();
    }
    for component in components {
        match component {
            Component::Prefix(..) => {}
            Component::RootDir => {}
            Component::CurDir => {}
            Component::ParentDir => {
                ret.pop();
            }
            Component::Normal(c) => {
                ret.push(c);
            }
        }
    }
    ret
}

fn resolve_path(input: &str, current_folder: &Path) -> Result<PathBuf, String> {
    let path = Path::new(input);
    
    let resolved = if path.is_absolute() {
        path.to_path_buf()
    } else {
        current_folder.join(path)
    };
    
    let normalized = normalize_path(&resolved);
    
    let final_path = if normalized.exists() {
        match normalized.canonicalize() {
            Ok(p) => p,
            Err(_) => normalized,
        }
    } else {
        normalized
    };
    
    if !final_path.exists() || !final_path.is_file() {
        let entered_path = input;
        let current_folder_str = current_folder.to_string_lossy();
        let resolved_path_str = final_path.to_string_lossy();
        
        let last_component = current_folder.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let mut hint_suggestion = String::new();
        if !last_component.is_empty() {
            let prefix_dir = format!("{}/", last_component);
            let prefix_dir_back = format!("{}\\", last_component);
            if entered_path.starts_with(&prefix_dir) {
                hint_suggestion = entered_path[prefix_dir.len()..].to_string();
            } else if entered_path.starts_with(&prefix_dir_back) {
                hint_suggestion = entered_path[prefix_dir_back.len()..].to_string();
            } else if entered_path == last_component {
                hint_suggestion = ".".to_string();
            }
        }
        
        let hint = if !hint_suggestion.is_empty() {
            format!("Try {}, change the current folder, or enter an absolute path.", hint_suggestion)
        } else {
            "Try a different relative path, change the current folder, or enter an absolute path.".to_string()
        };
        
        return Err(format!(
            "Could not read file.\n\n\
             Entered path: {}\n\
             Current folder: {}\n\
             Resolved path: {}\n\n\
             Hint: {}",
            entered_path, current_folder_str, resolved_path_str, hint
        ));
    }
    
    Ok(final_path)
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

    let mut lines = Vec::new();
    for line in reader.lines() {
        lines.push(line?);
    }

    let total_lines = lines.len();
    let mut non_empty_lines = 0;
    let mut import_lines = 0;
    let mut function_definitions = 0;
    let mut functions = Vec::new();

    // Iterate through lines to detect imports and overall non-empty lines
    for line in &lines {
        if !line.trim().is_empty() {
            non_empty_lines += 1;
        }
        let trimmed_start = line.trim_start();
        match language {
            Language::Rust => {
                if trimmed_start.starts_with("use ") || trimmed_start.starts_with("extern crate ") {
                    import_lines += 1;
                }
            }
            Language::JavaScript => {
                if trimmed_start.starts_with("import ") || trimmed_start.contains("require(") {
                    import_lines += 1;
                }
            }
            Language::Unsupported => {}
        }
    }

    // Parse functions if the language is supported
    match language {
        Language::Rust | Language::JavaScript => {
            let mut line_idx = 0;
            while line_idx < lines.len() {
                let line = &lines[line_idx];
                let is_func = match language {
                    Language::Rust => is_rust_function_definition(line),
                    Language::JavaScript => is_javascript_function_definition(line),
                    Language::Unsupported => false,
                };

                if is_func {
                    function_definitions += 1;
                    let start_line = line_idx + 1;
                    
                    // Parse function name
                    let name = match language {
                        Language::Rust => extract_rust_function_name(line).unwrap_or_else(|| format!("fn_at_{}", start_line)),
                        Language::JavaScript => extract_js_function_name(line).unwrap_or_else(|| format!("fn_at_{}", start_line)),
                        _ => format!("fn_at_{}", start_line),
                    };

                    // Trace function body to find its ending line.
                    // We need to count braces.
                    let mut brace_depth = 0;
                    let mut found_start_brace = false;
                    let mut current_idx = line_idx;
                    
                    // Scan characters from current line onwards
                    while current_idx < lines.len() {
                        let cur_line = &lines[current_idx];
                        
                        // Simple parser for characters in the line to ignore strings/comments
                        let mut chars = cur_line.chars().peekable();
                        let mut in_single_quote = false;
                        let mut in_double_quote = false;
                        let mut in_backtick = false;
                        let mut in_line_comment = false;
                        let mut escaped = false;
                        
                        while let Some(c) = chars.next() {
                            if escaped {
                                escaped = false;
                                continue;
                            }
                            if in_line_comment {
                                break;
                            }
                            match c {
                                '\\' => {
                                    escaped = true;
                                }
                                '/' if !in_single_quote && !in_double_quote && !in_backtick => {
                                    if let Some('/') = chars.peek() {
                                        in_line_comment = true;
                                    }
                                }
                                '\'' if !in_double_quote && !in_backtick => {
                                    in_single_quote = !in_single_quote;
                                }
                                '"' if !in_single_quote && !in_backtick => {
                                    in_double_quote = !in_double_quote;
                                }
                                '`' if !in_single_quote && !in_double_quote => {
                                    in_backtick = !in_backtick;
                                }
                                '{' if !in_single_quote && !in_double_quote && !in_backtick => {
                                    brace_depth += 1;
                                    found_start_brace = true;
                                }
                                '}' if !in_single_quote && !in_double_quote && !in_backtick => {
                                    if brace_depth > 0 {
                                        brace_depth -= 1;
                                    }
                                }
                                _ => {}
                            }
                        }

                        if found_start_brace && brace_depth == 0 {
                            break;
                        }
                        current_idx += 1;
                    }

                    let end_line = if current_idx < lines.len() {
                        current_idx + 1
                    } else {
                        lines.len()
                    };

                    let total_func_lines = end_line - start_line + 1;
                    let mut non_empty_func_lines = 0;
                    for l in start_line..=end_line {
                        if l <= lines.len() && !lines[l - 1].trim().is_empty() {
                            non_empty_func_lines += 1;
                        }
                    }

                    functions.push(FunctionResult {
                        name,
                        start_line,
                        end_line,
                        total_lines: total_func_lines,
                        non_empty_lines: non_empty_func_lines,
                    });

                    // Advance line_idx to end of function to avoid scanning inner functions
                    line_idx = end_line;
                } else {
                    line_idx += 1;
                }
            }
        }
        _ => {}
    }

    let analyzed_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    Ok(AnalysisResult {
        source_file: filename.to_string(),
        language: language.as_str().to_string(),
        analyzed_at,
        total_lines,
        non_empty_lines,
        import_lines,
        scope: None,
        function_definitions,
        functions,
    })
}

fn extract_rust_function_name(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let parts: Vec<&str> = trimmed.split_whitespace().collect();
    let fn_idx = parts.iter().position(|&s| s == "fn")?;
    if fn_idx + 1 < parts.len() {
        let name_part = parts[fn_idx + 1];
        let name = name_part.split('(').next()?;
        Some(name.trim().to_string())
    } else {
        None
    }
}

fn extract_js_function_name(line: &str) -> Option<String> {
    if let Some(name) = is_js_method(line) {
        return Some(name);
    }

    let line_without_comment = match line.find("//") {
        Some(idx) => &line[..idx],
        None => line,
    };
    let trimmed = line_without_comment.trim_start();
    
    // Traditional: function name(...)
    if trimmed.contains("function ") || trimmed.contains("function(") {
        let parts: Vec<&str> = trimmed.split_whitespace().collect();
        let func_idx = parts.iter().position(|&s| s.starts_with("function"))?;
        if func_idx + 1 < parts.len() {
            let name_part = parts[func_idx + 1];
            if let Some(name) = name_part.split('(').next() {
                if !name.trim().is_empty() {
                    return Some(name.trim().to_string());
                }
            }
        }
        // Anonymous or default export
        return Some("anonymous".to_string());
    }

    // Arrow functions: const multiply = (a, b) => a * b;
    if trimmed.starts_with("const ") || trimmed.starts_with("let ") || trimmed.starts_with("var ") || trimmed.starts_with("export const ") || trimmed.starts_with("export let ") {
        let clean_trimmed = if trimmed.starts_with("export ") {
            trimmed["export ".len()..].trim_start()
        } else {
            trimmed
        };
        let parts: Vec<&str> = clean_trimmed.split_whitespace().collect();
        if parts.len() >= 2 {
            let name = parts[1];
            return Some(name.trim().to_string());
        }
    }

    None
}

fn save_analysis_result_log(result: &AnalysisResult, output_path: Option<&str>) -> io::Result<String> {
    let final_path = match output_path {
        Some(path) => {
            if is_directory_path(path) {
                let timestamp = get_fs_safe_timestamp();
                let source_name = sanitize_filename(&result.source_file);
                let func_part = if let Some(ref scope) = result.scope {
                    format!("_{}", scope.name)
                } else {
                    "".to_string()
                };
                let mut path_buf = Path::new(path).to_path_buf();
                path_buf.push(format!("{}{}-{}.log", source_name, func_part, timestamp));
                path_buf.to_string_lossy().to_string()
            } else if path.ends_with(".log") {
                path.to_string()
            } else {
                let timestamp = get_fs_safe_timestamp();
                let source_name = sanitize_filename(&result.source_file);
                let func_part = if let Some(ref scope) = result.scope {
                    format!("_{}", scope.name)
                } else {
                    "".to_string()
                };
                format!("logs/{}{}-{}.log", source_name, func_part, timestamp)
            }
        }
        None => {
            let timestamp = get_fs_safe_timestamp();
            let source_name = sanitize_filename(&result.source_file);
            let func_part = if let Some(ref scope) = result.scope {
                format!("_{}", scope.name)
            } else {
                "".to_string()
            };
            format!("logs/{}{}-{}.log", source_name, func_part, timestamp)
        }
    };

    if let Some(parent) = Path::new(&final_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .open(&final_path)?;
    writeln!(file, "source_file={}", result.source_file)?;
    writeln!(file, "language={}", result.language)?;
    writeln!(file, "analyzed_at={}", result.analyzed_at)?;
    writeln!(file, "total_lines={}", result.total_lines)?;
    writeln!(file, "non_empty_lines={}", result.non_empty_lines)?;
    writeln!(file, "import_lines={}", result.import_lines)?;
    writeln!(file, "function_definitions={}", result.function_definitions)?;
    if let Some(ref scope) = result.scope {
        writeln!(
            file,
            "scope={}:{}:{}:{}:{}",
            scope.scope_type, scope.name, scope.file, scope.start_line, scope.end_line
        )?;
    }
    for func in &result.functions {
        writeln!(
            file,
            "function={}:{}:{}:{}:{}",
            func.name, func.start_line, func.end_line, func.total_lines, func.non_empty_lines
        )?;
    }
    writeln!(file)?;
    Ok(final_path)
}

/// Saves an `AnalysisResult` to a structured key-value log file.
#[allow(dead_code)]
fn save_analysis_result(result: &AnalysisResult, log_filename: &str) -> io::Result<()> {
    save_analysis_result_log(result, Some(log_filename)).map(|_| ())
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
    let mut scope = None;
    let mut functions = Vec::new();

    let mut blocks = Vec::new();
    let mut current_block = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            if !current_block.is_empty() {
                blocks.push(current_block);
                current_block = Vec::new();
            }
        } else {
            current_block.push(line);
        }
    }
    if !current_block.is_empty() {
        blocks.push(current_block);
    }

    let last_block = blocks.last().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("Log file '{}' is empty", log_filename),
        )
    })?;

    for (line_idx, line) in last_block.iter().enumerate() {
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
            "scope" => {
                let parts: Vec<&str> = val.split(':').collect();
                if parts.len() == 5 {
                    scope = Some(ScopeInfo {
                        scope_type: parts[0].to_string(),
                        name: parts[1].to_string(),
                        file: parts[2].to_string(),
                        start_line: parts[3].parse().unwrap_or(0),
                        end_line: parts[4].parse().unwrap_or(0),
                    });
                }
            }
            "function" => {
                let f_parts: Vec<&str> = val.split(':').collect();
                if f_parts.len() != 5 {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("Malformed function entry in '{}': {}", log_filename, val),
                    ));
                }
                let name = f_parts[0].to_string();
                let start_line = f_parts[1].parse::<usize>().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "Invalid start_line in function log")
                })?;
                let end_line = f_parts[2].parse::<usize>().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "Invalid end_line in function log")
                })?;
                let total_lines = f_parts[3].parse::<usize>().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "Invalid total_lines in function log")
                })?;
                let non_empty_lines = f_parts[4].parse::<usize>().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "Invalid non_empty_lines in function log")
                })?;
                functions.push(FunctionResult {
                    name,
                    start_line,
                    end_line,
                    total_lines,
                    non_empty_lines,
                });
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
        scope,
        function_definitions: function_definitions.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "Missing required field 'function_definitions' in '{}'",
                    log_filename
                ),
            )
        })?,
        functions,
    })
}

/// Compares a newly analyzed source file with a prior analysis log file.
fn compare_current_with_log(source_file: &str, log_filename: &str, function_name: Option<&str>) -> io::Result<()> {
    let current_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let resolved_source = match resolve_path(source_file, &current_dir) {
        Ok(p) => p,
        Err(e) => return Err(io::Error::new(io::ErrorKind::NotFound, e)),
    };
    let resolved_log = match resolve_path(log_filename, &current_dir) {
        Ok(p) => p,
        Err(e) => return Err(io::Error::new(io::ErrorKind::NotFound, e)),
    };
    let source_str = resolved_source.to_string_lossy();
    let log_str = resolved_log.to_string_lossy();
    let mut old = parse_analysis_result(&log_str)?;
    let mut new = analyze_file(&source_str)?;

    if let Some(func_name) = function_name {
        if let Some(ref scope) = old.scope {
            if scope.name != func_name {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("Log file '{}' is scoped to function '{}', but requested '{}'", log_filename, scope.name, func_name),
                ));
            }
        } else {
            apply_function_scope(&mut old, func_name)?;
        }
        apply_function_scope(&mut new, func_name)?;
    }

    println!("Comparison: {} vs {}", source_file, log_filename);
    println!();
    println!("Source file:          {} (current) vs {} (baseline)", new.source_file, old.source_file);
    println!("Language:             {} (current) vs {} (baseline)", new.language, old.language);
    println!("Analyzed at:          {} (current) vs {} (baseline)", new.analyzed_at, old.analyzed_at);
    println!();

    print_comparison_metric("Total lines:", old.total_lines, new.total_lines);
    print_comparison_metric("Non-empty lines:", old.non_empty_lines, new.non_empty_lines);
    print_comparison_metric("Import lines:", old.import_lines, new.import_lines);
    print_comparison_metric("Function definitions:", old.function_definitions, new.function_definitions);
    println!();

    println!("Function-Level Comparisons:");
    
    let mut changed = Vec::new();
    let mut added = Vec::new();
    let mut removed = Vec::new();

    for new_func in &new.functions {
        if let Some(old_func) = old.functions.iter().find(|f| f.name == new_func.name) {
            changed.push((old_func.clone(), new_func.clone()));
        } else {
            added.push(new_func.clone());
        }
    }

    for old_func in &old.functions {
        if !new.functions.iter().any(|f| f.name == old_func.name) {
            removed.push(old_func.clone());
        }
    }

    if !changed.is_empty() {
        println!("  Changed/Unchanged Functions:");
        for (old_f, new_f) in &changed {
            let diff = (new_f.total_lines as isize) - (old_f.total_lines as isize);
            let status = if diff > 0 {
                format!("+{}, increased", diff)
            } else if diff < 0 {
                format!("-{}, decreased", -diff)
            } else {
                "unchanged".to_string()
            };
            println!(
                "    - {}: {} → {} lines ({})",
                new_f.name, old_f.total_lines, new_f.total_lines, status
            );
        }
    }

    if !added.is_empty() {
        println!("  Added Functions:");
        for f in &added {
            println!("    - {} (lines {}-{}, total: {})", f.name, f.start_line, f.end_line, f.total_lines);
        }
    }

    if !removed.is_empty() {
        println!("  Removed Functions:");
        for f in &removed {
            println!("    - {} (total: {})", f.name, f.total_lines);
        }
    }

    if changed.is_empty() && added.is_empty() && removed.is_empty() {
        println!("  No function-level metrics to compare (or file does not contain functions).");
    }

    Ok(())
}

fn print_comparison_metric(label: &str, old_val: usize, new_val: usize) {
    let diff = (new_val as isize) - (old_val as isize);
    if diff > 0 {
        println!(
            "{:<22} {:>3} → {:<4} (+{}, increased)",
            label, old_val, new_val, diff
        );
    } else if diff < 0 {
        println!(
            "{:<22} {:>3} → {:<4} (-{}, decreased)",
            label, old_val, new_val, -diff
        );
    } else {
        println!(
            "{:<22} {:>3} → {:<4} (unchanged)",
            label, old_val, new_val
        );
    }
}

fn is_js_method(line: &str) -> Option<String> {
    let line_without_comment = match line.find("//") {
        Some(idx) => &line[..idx],
        None => line,
    };
    let mut trimmed = line_without_comment.trim();
    if trimmed.starts_with("async ") {
        trimmed = trimmed["async ".len()..].trim_start();
    }
    
    let open_paren = trimmed.find('(')?;
    let name = trimmed[..open_paren].trim();
    
    if name.is_empty() || !name.chars().next()?.is_alphabetic() && name.chars().next()? != '_' {
        return None;
    }
    if name.chars().any(|c| !c.is_alphanumeric() && c != '_') {
        return None;
    }
    
    let keywords = ["if", "for", "while", "switch", "catch", "function", "return", "await", "import", "export"];
    if keywords.contains(&name) {
        return None;
    }
    
    if trimmed[open_paren..].contains('{') {
        return Some(name.to_string());
    }
    
    None
}

fn is_directory_path(path_str: &str) -> bool {
    let path = Path::new(path_str);
    if path.is_dir() {
        return true;
    }
    if path_str.ends_with('/') || path_str.ends_with('\\') {
        return true;
    }
    if path.extension().is_none() {
        return true;
    }
    false
}

fn apply_function_scope(result: &mut AnalysisResult, function_name: &str) -> io::Result<()> {
    let matches: Vec<FunctionResult> = result.functions.iter().filter(|f| f.name == function_name).cloned().collect();
    if matches.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("Function '{}' not found in '{}'", function_name, result.source_file),
        ));
    }
    if matches.len() > 1 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("Function '{}' is ambiguous in '{}' (multiple definitions found)", function_name, result.source_file),
        ));
    }
    let target = matches[0].clone();

    result.scope = Some(ScopeInfo {
        scope_type: "function".to_string(),
        name: target.name.clone(),
        file: result.source_file.clone(),
        start_line: target.start_line,
        end_line: target.end_line,
    });

    result.total_lines = target.total_lines;
    result.non_empty_lines = target.non_empty_lines;
    result.import_lines = 0;
    result.function_definitions = 1;
    result.functions = vec![target];

    Ok(())
}

fn is_rust_function_definition(line: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with("fn") {
        return false;
    }
    match trimmed.chars().nth(2) {
        None => true,
        Some(c) => !c.is_alphanumeric() && c != '_',
    }
}

fn is_javascript_function_definition(line: &str) -> bool {
    if is_js_method(line).is_some() {
        return true;
    }

    let line_without_comment = match line.find("//") {
        Some(idx) => &line[..idx],
        None => line,
    };
    
    let mut trimmed = line_without_comment.trim_start();
    
    if trimmed.starts_with("export default ") {
        trimmed = trimmed["export default ".len()..].trim_start();
    } else if trimmed.starts_with("export ") {
        trimmed = trimmed["export ".len()..].trim_start();
    }

    let is_js_word = |t: &str, word: &str| -> bool {
        if !t.starts_with(word) {
            return false;
        }
        match t.chars().nth(word.len()) {
            None => true,
            Some(c) => !c.is_alphanumeric() && c != '_',
        }
    };

    if is_js_word(trimmed, "function") {
        return true;
    }
    if is_js_word(trimmed, "async") {
        let after_async = trimmed["async".len()..].trim_start();
        if is_js_word(after_async, "function") {
            return true;
        }
    }

    if is_js_word(trimmed, "const") || is_js_word(trimmed, "let") || is_js_word(trimmed, "var") {
        if let Some(eq_idx) = trimmed.find('=') {
            let after_eq = &trimmed[eq_idx + 1..];
            if has_arrow_operator(after_eq) {
                return true;
            }
        }
    }

    false
}

fn has_arrow_operator(s: &str) -> bool {
    let mut chars = s.chars().peekable();
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let mut in_backtick = false;
    let mut escaped = false;

    while let Some(c) = chars.next() {
        if escaped {
            escaped = false;
            continue;
        }
        match c {
            '\\' => {
                escaped = true;
            }
            '\'' if !in_double_quote && !in_backtick => {
                in_single_quote = !in_single_quote;
            }
            '"' if !in_single_quote && !in_backtick => {
                in_double_quote = !in_double_quote;
            }
            '`' if !in_single_quote && !in_double_quote => {
                in_backtick = !in_backtick;
            }
            '=' if !in_single_quote && !in_double_quote && !in_backtick => {
                if let Some(&'>') = chars.peek() {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct LogMetadata {
    label: String,
    source: String,
    total_lines: usize,
    error_count: usize,
    warn_count: usize,
    info_count: usize,
    avg_latency_ms: Option<f64>,
    max_latency_ms: Option<f64>,
    min_latency_ms: Option<f64>,
    latency_values: Vec<f64>,
    is_code_analyzer_log: bool,
    code_total_lines: Option<usize>,
    code_non_empty_lines: Option<usize>,
    code_import_lines: Option<usize>,
    code_function_definitions: Option<usize>,
}

fn parse_log_content(content: &str, label: String, source: String) -> LogMetadata {
    let mut is_code = false;
    let mut code_total_lines = None;
    let mut code_non_empty_lines = None;
    let mut code_import_lines = None;
    let mut code_function_definitions = None;

    if content.trim_start().starts_with('{') {
        if let Ok(analyze_res) = serde_json::from_str::<AnalyzeJsonResult>(content) {
            is_code = true;
            code_total_lines = Some(analyze_res.result.total_lines);
            code_non_empty_lines = Some(analyze_res.result.non_empty_lines);
            code_import_lines = Some(analyze_res.result.import_lines);
            code_function_definitions = Some(analyze_res.result.function_definitions);
        } else if let Ok(analysis_res) = serde_json::from_str::<AnalysisResult>(content) {
            is_code = true;
            code_total_lines = Some(analysis_res.total_lines);
            code_non_empty_lines = Some(analysis_res.non_empty_lines);
            code_import_lines = Some(analysis_res.import_lines);
            code_function_definitions = Some(analysis_res.function_definitions);
        }
    } else if content.contains("source_file=") && content.contains("language=") && content.contains("total_lines=") {
        is_code = true;
        for line in content.lines() {
            let parts: Vec<&str> = line.splitn(2, '=').collect();
            if parts.len() == 2 {
                let key = parts[0].trim();
                let val = parts[1].trim();
                match key {
                    "total_lines" => code_total_lines = val.parse().ok(),
                    "non_empty_lines" => code_non_empty_lines = val.parse().ok(),
                    "import_lines" => code_import_lines = val.parse().ok(),
                    "function_definitions" => code_function_definitions = val.parse().ok(),
                    _ => {}
                }
            }
        }
    }

    let mut total_lines = 0;
    let mut error_count = 0;
    let mut warn_count = 0;
    let mut info_count = 0;
    let mut latency_values = Vec::new();

    for line in content.lines() {
        total_lines += 1;
        let line_lower = line.to_lowercase();

        if line_lower.contains("error") || line_lower.contains("fatal") || line_lower.contains("fail") || line_lower.contains("exception") {
            error_count += 1;
        } else if line_lower.contains("warn") {
            warn_count += 1;
        } else if line_lower.contains("info") {
            info_count += 1;
        }

        if let Some(ms_idx) = line_lower.find("ms") {
            let before_ms = &line_lower[..ms_idx];
            let mut start_idx = before_ms.len();
            for c in before_ms.chars().rev() {
                if c.is_ascii_digit() || c == '.' {
                    start_idx -= c.len_utf8();
                } else {
                    break;
                }
            }
            if start_idx < before_ms.len() {
                if let Ok(val) = before_ms[start_idx..].parse::<f64>() {
                    latency_values.push(val);
                }
            }
        } else {
            for key in &["latency", "duration"] {
                if let Some(key_idx) = line_lower.find(key) {
                    let after_key = &line_lower[key_idx + key.len()..];
                    let mut start_idx = 0;
                    let mut chars = after_key.chars().peekable();
                    while let Some(&c) = chars.peek() {
                        if c == '=' || c == ':' || c.is_whitespace() {
                            start_idx += c.len_utf8();
                            chars.next();
                        } else {
                            break;
                        }
                    }
                    let mut len = 0;
                    for c in after_key[start_idx..].chars() {
                        if c.is_ascii_digit() || c == '.' {
                            len += c.len_utf8();
                        } else {
                            break;
                        }
                    }
                    if len > 0 {
                        if let Ok(val) = after_key[start_idx..start_idx + len].parse::<f64>() {
                            let is_seconds = after_key[start_idx + len..].trim_start().starts_with('s') || (key == &"duration" && val < 50.0);
                            let normalized_val = if is_seconds { val * 1000.0 } else { val };
                            latency_values.push(normalized_val);
                        }
                    }
                }
            }
        }
    }

    let min_latency_ms = latency_values.iter().copied().fold(None, |min, val| Some(min.map_or(val, |m: f64| m.min(val))));
    let max_latency_ms = latency_values.iter().copied().fold(None, |max, val| Some(max.map_or(val, |m: f64| m.max(val))));
    let avg_latency_ms = if latency_values.is_empty() {
        None
    } else {
        Some(latency_values.iter().sum::<f64>() / latency_values.len() as f64)
    };

    LogMetadata {
        label,
        source,
        total_lines,
        error_count,
        warn_count,
        info_count,
        avg_latency_ms,
        max_latency_ms,
        min_latency_ms,
        latency_values,
        is_code_analyzer_log: is_code,
        code_total_lines,
        code_non_empty_lines,
        code_import_lines,
        code_function_definitions,
    }
}

fn print_summary(logs: &[LogMetadata], prompt: &str) {
    if logs.is_empty() {
        println!("No logs to summarize.");
        return;
    }
    let prompt_lower = prompt.to_lowercase();
    if prompt_lower.contains("latency") || prompt_lower.contains("duration") || prompt_lower.contains("time") {
        println!("Latency analysis across {} logs:", logs.len());
        for log in logs {
            match log.avg_latency_ms {
                Some(avg) => {
                    println!(
                        "  - {} (Avg: {:.1}ms, Min: {:.1}ms, Max: {:.1}ms)",
                        log.label,
                        avg,
                        log.min_latency_ms.unwrap_or(0.0),
                        log.max_latency_ms.unwrap_or(0.0)
                    );
                }
                None => println!("  - {}: No latency data found.", log.label),
            }
        }
    } else if prompt_lower.contains("error") || prompt_lower.contains("anomaly") || prompt_lower.contains("fail") {
        println!("Error and anomaly analysis across {} logs:", logs.len());
        let mut max_err_log = &logs[0];
        for log in logs {
            println!("  - {}: {} errors found across {} total lines.", log.label, log.error_count, log.total_lines);
            if log.error_count > max_err_log.error_count {
                max_err_log = log;
            }
        }
        if max_err_log.error_count > 0 {
            println!("Key finding: Log '{}' has the highest error frequency.", max_err_log.label);
        } else {
            println!("Key finding: No errors detected in any of the logs.");
        }
    } else {
        println!("General comparison of {} logs:", logs.len());
        for log in logs {
            if log.is_code_analyzer_log {
                println!(
                    "  - {} (Code Log): {} lines, {} functions defined.",
                    log.label,
                    log.code_total_lines.unwrap_or(0),
                    log.code_function_definitions.unwrap_or(0)
                );
            } else {
                println!(
                    "  - {}: {} lines, {} errors, {} warnings, {} info messages.",
                    log.label, log.total_lines, log.error_count, log.warn_count, log.info_count
                );
            }
        }
    }
}

fn print_comparison_table(logs: &[LogMetadata], prompt: &str) {
    if logs.is_empty() {
        println!("No logs to display.");
        return;
    }
    
    let prompt_lower = prompt.to_lowercase();
    let mut rows = Vec::new();
    
    rows.push(("Label".to_string(), logs.iter().map(|l| l.label.clone()).collect::<Vec<_>>()));
    rows.push(("Source".to_string(), logs.iter().map(|l| l.source.clone()).collect::<Vec<_>>()));

    if prompt_lower.contains("latency") || prompt_lower.contains("duration") || prompt_lower.contains("time") {
        rows.push(("Min Latency".to_string(), logs.iter().map(|l| l.min_latency_ms.map_or("N/A".to_string(), |v| format!("{:.1}ms", v))).collect::<Vec<_>>()));
        rows.push(("Avg Latency".to_string(), logs.iter().map(|l| l.avg_latency_ms.map_or("N/A".to_string(), |v| format!("{:.1}ms", v))).collect::<Vec<_>>()));
        rows.push(("Max Latency".to_string(), logs.iter().map(|l| l.max_latency_ms.map_or("N/A".to_string(), |v| format!("{:.1}ms", v))).collect::<Vec<_>>()));
    } else if prompt_lower.contains("error") || prompt_lower.contains("anomaly") || prompt_lower.contains("fail") {
        rows.push(("Total Lines".to_string(), logs.iter().map(|l| l.total_lines.to_string()).collect::<Vec<_>>()));
        rows.push(("Error Count".to_string(), logs.iter().map(|l| l.error_count.to_string()).collect::<Vec<_>>()));
        rows.push(("Warn Count".to_string(), logs.iter().map(|l| l.warn_count.to_string()).collect::<Vec<_>>()));
        rows.push(("Error Rate".to_string(), logs.iter().map(|l| {
            if l.total_lines > 0 {
                format!("{:.2}%", (l.error_count as f64 / l.total_lines as f64) * 100.0)
            } else {
                "0.00%".to_string()
            }
        }).collect::<Vec<_>>()));
    } else if prompt_lower.contains("code") || logs.iter().any(|l| l.is_code_analyzer_log) {
        rows.push(("Code Lines".to_string(), logs.iter().map(|l| l.code_total_lines.map_or("N/A".to_string(), |v| v.to_string())).collect::<Vec<_>>()));
        rows.push(("Code Non-Empty".to_string(), logs.iter().map(|l| l.code_non_empty_lines.map_or("N/A".to_string(), |v| v.to_string())).collect::<Vec<_>>()));
        rows.push(("Code Import Lines".to_string(), logs.iter().map(|l| l.code_import_lines.map_or("N/A".to_string(), |v| v.to_string())).collect::<Vec<_>>()));
        rows.push(("Code Fns Defined".to_string(), logs.iter().map(|l| l.code_function_definitions.map_or("N/A".to_string(), |v| v.to_string())).collect::<Vec<_>>()));
    } else {
        rows.push(("Total Lines".to_string(), logs.iter().map(|l| l.total_lines.to_string()).collect::<Vec<_>>()));
        rows.push(("Error Count".to_string(), logs.iter().map(|l| l.error_count.to_string()).collect::<Vec<_>>()));
        rows.push(("Warn Count".to_string(), logs.iter().map(|l| l.warn_count.to_string()).collect::<Vec<_>>()));
        rows.push(("Avg Latency".to_string(), logs.iter().map(|l| l.avg_latency_ms.map_or("N/A".to_string(), |v| format!("{:.1}ms", v))).collect::<Vec<_>>()));
    }

    let mut name_width = 18;
    for (name, _) in &rows {
        name_width = name_width.max(name.len());
    }

    let mut col_widths = vec![name_width];
    for col_idx in 0..logs.len() {
        let mut width = 10;
        for (_, cols) in &rows {
            width = width.max(cols[col_idx].len());
        }
        col_widths.push(width);
    }

    let separator = col_widths.iter().map(|w| "-".repeat(*w)).collect::<Vec<_>>().join("-+-");
    println!("+-{}-+", separator);
    
    for (name, cols) in rows {
        print!("| {:<width$} |", name, width = col_widths[0]);
        for (col_idx, val) in cols.iter().enumerate() {
            print!(" {:<width$} |", val, width = col_widths[col_idx + 1]);
        }
        println!();
        println!("+-{}-+", separator);
    }
}

fn draw_bar_chart(labels: &[String], values: &[f64], title: &str) {
    println!("\n=== {} ===", title);
    if values.is_empty() {
        println!("No numeric data to display.");
        return;
    }
    let max_val = values.iter().copied().fold(0.0, f64::max);
    let max_label_len = labels.iter().map(|l| l.len()).max().unwrap_or(0);
    
    for (label, &val) in labels.iter().zip(values.iter()) {
        let bar_width = if max_val > 0.0 {
            ((val / max_val) * 30.0) as usize
        } else {
            0
        };
        let bar = "█".repeat(bar_width);
        println!(
            "  {:<width$} | {:<30} ({:.1})",
            label,
            bar,
            val,
            width = max_label_len
        );
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AnalyzeJsonResult {
    metadata: AnalyzeMetadata,
    result: AnalysisResult,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct AnalyzeMetadata {
    command: String,
    timestamp: String,
    input_file: String,
    tool_version: String,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CompareJsonResult {
    metadata: CompareMetadata,
    logs: Vec<LogMetadata>,
    summary: String,
    comparison_table: Vec<Vec<String>>,
    graph_data: GraphDataJson,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct CompareMetadata {
    command: String,
    timestamp: String,
    input_files: Vec<String>,
    tool_version: String,
    prompt: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct GraphDataJson {
    title: String,
    labels: Vec<String>,
    values: Vec<f64>,
}

fn get_fs_safe_timestamp() -> String {
    let now = Utc::now();
    now.format("%Y-%m-%dT%H-%M-%SZ").to_string()
}

fn sanitize_filename(name: &str) -> String {
    let path = Path::new(name);
    let stem = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(name);
        
    let mut safe = String::new();
    for c in stem.chars() {
        if c.is_alphanumeric() || c == '_' || c == '-' || c == '.' {
            safe.push(c);
        } else {
            safe.push('_');
        }
    }
    if safe.is_empty() {
        "unnamed".to_string()
    } else {
        safe
    }
}

fn save_analysis_result_json(result: &AnalysisResult, output_path: Option<&str>) -> io::Result<String> {
    let final_path = match output_path {
        Some(path) => {
            if is_directory_path(path) {
                let timestamp = get_fs_safe_timestamp();
                let source_name = sanitize_filename(&result.source_file);
                let func_part = if let Some(ref scope) = result.scope {
                    format!("_{}", scope.name)
                } else {
                    "".to_string()
                };
                let mut path_buf = Path::new(path).to_path_buf();
                path_buf.push(format!("{}{}-{}.json", source_name, func_part, timestamp));
                path_buf.to_string_lossy().to_string()
            } else if path.ends_with(".json") {
                path.to_string()
            } else {
                let timestamp = get_fs_safe_timestamp();
                let source_name = sanitize_filename(&result.source_file);
                let func_part = if let Some(ref scope) = result.scope {
                    format!("_{}", scope.name)
                } else {
                    "".to_string()
                };
                format!("analysis-results/analyze/{}{}-{}.json", source_name, func_part, timestamp)
            }
        }
        None => {
            let timestamp = get_fs_safe_timestamp();
            let source_name = sanitize_filename(&result.source_file);
            let func_part = if let Some(ref scope) = result.scope {
                format!("_{}", scope.name)
            } else {
                "".to_string()
            };
            format!("analysis-results/analyze/{}{}-{}.json", source_name, func_part, timestamp)
        }
    };

    if let Some(parent) = Path::new(&final_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let json_res = AnalyzeJsonResult {
        metadata: AnalyzeMetadata {
            command: "analyze".to_string(),
            timestamp: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            input_file: result.source_file.clone(),
            tool_version: env!("CARGO_PKG_VERSION").to_string(),
        },
        result: result.clone(),
    };

    let serialized = serde_json::to_string_pretty(&json_res)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(&final_path, serialized)?;
    Ok(final_path)
}

fn save_compare_result_json(
    metadata: CompareMetadata,
    logs: Vec<LogMetadata>,
    summary: String,
    comparison_table: Vec<Vec<String>>,
    graph_data: GraphDataJson,
    output_path: Option<&str>,
) -> io::Result<String> {
    let final_path = match output_path {
        Some(path) => {
            if is_directory_path(path) {
                let timestamp = metadata.timestamp.replace(':', "-");
                let mut name_parts = Vec::new();
                for log in &logs {
                    name_parts.push(sanitize_filename(&log.label));
                }
                let logs_vs_str = name_parts.join("-vs-");
                let mut path_buf = Path::new(path).to_path_buf();
                path_buf.push(format!("{}-{}.json", logs_vs_str, timestamp));
                path_buf.to_string_lossy().to_string()
            } else {
                path.to_string()
            }
        }
        None => {
            let timestamp = metadata.timestamp.replace(':', "-");
            let mut name_parts = Vec::new();
            for log in &logs {
                name_parts.push(sanitize_filename(&log.label));
            }
            let logs_vs_str = name_parts.join("-vs-");
            format!("analysis-results/compare/{}-{}.json", logs_vs_str, timestamp)
        }
    };

    if let Some(parent) = Path::new(&final_path).parent() {
        std::fs::create_dir_all(parent)?;
    }

    let json_res = CompareJsonResult {
        metadata,
        logs,
        summary,
        comparison_table,
        graph_data,
    };

    let serialized = serde_json::to_string_pretty(&json_res)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    std::fs::write(&final_path, serialized)?;
    Ok(final_path)
}

// ==========================================
// TUI Implementation
// ==========================================

enum InputMode {
    Normal,
    Editing,
}

enum Screen {
    MainMenu,
    AnalyzeInputFiles {
        selected_files: Vec<String>,
    },
    AnalyzeInputFunction {
        files: Vec<String>,
    },
    AnalyzeResult {
        results: Vec<AnalysisResult>,
        error: Option<String>,
    },
    CompareSelection {
        logs: Vec<PathBuf>,
        selected: Vec<bool>,
        state: ListState,
    },
    CompareInputFunction {
        selected_logs: Vec<PathBuf>,
    },
    CompareResult {
        selected_logs: Vec<PathBuf>,
        function_name: Option<String>,
        comparison_text: String,
        error: Option<String>,
    },
    ListLogs {
        logs: Vec<PathBuf>,
        state: ListState,
        viewing_log: Option<String>,
        confirm_delete: Option<PathBuf>,
    },
    ChangeFolder,
    Help,
}

struct AppState {
    current_dir: PathBuf,
    screen: Screen,
    input_value: String,
    input_mode: InputMode,
    status_message: String,
}

fn scan_analyzable_files(dir: &Path, base_dir: &Path) -> Vec<String> {
    let mut results = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if path.is_dir() {
                if name == "target" || name == "node_modules" || name == ".git" || name == "logs" {
                    continue;
                }
                results.extend(scan_analyzable_files(&path, base_dir));
            } else if path.is_file() {
                if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
                    if ext == "rs" || ext == "js" || ext == "mjs" || ext == "cjs" {
                        if let Ok(rel) = path.strip_prefix(base_dir) {
                            results.push(rel.to_string_lossy().to_string());
                        }
                    }
                }
            }
        }
    }
    results.sort();
    results
}

fn longest_common_prefix(words: &[String]) -> String {
    if words.is_empty() {
        return String::new();
    }
    let first = &words[0];
    let mut min_len = first.len();
    for w in words.iter().skip(1) {
        min_len = min_len.min(w.len());
    }
    let mut prefix = String::new();
    for i in 0..min_len {
        let c = first.chars().nth(i).unwrap();
        if words.iter().all(|w| w.chars().nth(i) == Some(c)) {
            prefix.push(c);
        } else {
            break;
        }
    }
    prefix
}

fn extract_functions_from_files(files: &[String]) -> Vec<(String, String)> {
    let mut results = Vec::new();
    for f in files {
        if let Ok(res) = analyze_file(f) {
            for func in res.functions {
                results.push((func.name, f.clone()));
            }
        }
    }
    results
}

fn run_tui() -> Result<(), Box<dyn std::error::Error>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let current_dir = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let mut app = AppState {
        current_dir,
        screen: Screen::MainMenu,
        input_value: String::new(),
        input_mode: InputMode::Normal,
        status_message: String::new(),
    };

    let res = run_app(&mut terminal, &mut app);

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("TUI terminated with error: {:?}", err);
    }
    Ok(())
}

fn run_app<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut AppState,
) -> io::Result<()> {
    loop {
        terminal.draw(|f| ui(f, app))?;

        if event::poll(std::time::Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == event::KeyEventKind::Press {
                    match app.input_mode {
                        InputMode::Normal => {
                            match &mut app.screen {
                                Screen::MainMenu => match key.code {
                                    KeyCode::Char('a') | KeyCode::Char('A') => {
                                        app.screen = Screen::AnalyzeInputFiles { selected_files: Vec::new() };
                                        app.input_mode = InputMode::Editing;
                                        app.input_value.clear();
                                    }
                                    KeyCode::Char('c') | KeyCode::Char('C') => {
                                        let logs_dir = app.current_dir.join("logs");
                                        let logs = list_log_files(&logs_dir);
                                        let len = logs.len();
                                        app.screen = Screen::CompareSelection {
                                            logs,
                                            selected: vec![false; len],
                                            state: ListState::default(),
                                        };
                                    }
                                    KeyCode::Char('l') | KeyCode::Char('L') => {
                                        let logs_dir = app.current_dir.join("logs");
                                        let logs = list_log_files(&logs_dir);
                                        let mut state = ListState::default();
                                        if !logs.is_empty() {
                                            state.select(Some(0));
                                        }
                                        app.screen = Screen::ListLogs {
                                            logs,
                                            state,
                                            viewing_log: None,
                                            confirm_delete: None,
                                        };
                                    }
                                    KeyCode::Char('d') | KeyCode::Char('D') => {
                                        app.screen = Screen::ChangeFolder;
                                        app.input_mode = InputMode::Editing;
                                        app.input_value = app.current_dir.to_string_lossy().to_string();
                                    }
                                    KeyCode::Char('h') | KeyCode::Char('H') => {
                                        app.screen = Screen::Help;
                                    }
                                    KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                                        return Ok(());
                                    }
                                    _ => {}
                                },
                                Screen::AnalyzeResult { results, .. } => match key.code {
                                    KeyCode::Char('s') | KeyCode::Char('S') => {
                                        let logs_dir = app.current_dir.join("logs");
                                        let mut saved_paths = Vec::new();
                                        let mut err_msg = None;
                                        for res in results {
                                            match save_analysis_result_log(res, Some(&logs_dir.to_string_lossy())) {
                                                Ok(path) => saved_paths.push(path),
                                                Err(e) => {
                                                    err_msg = Some(format!("Error saving log: {}", e));
                                                    break;
                                                }
                                            }
                                        }
                                        if let Some(msg) = err_msg {
                                            app.status_message = msg;
                                        } else {
                                            app.status_message = format!("Saved {} log(s) to logs/", saved_paths.len());
                                        }
                                    }
                                    KeyCode::Char('b') | KeyCode::Char('B') | KeyCode::Esc => {
                                        app.screen = Screen::MainMenu;
                                        app.status_message.clear();
                                    }
                                    KeyCode::Char('q') | KeyCode::Char('Q') => {
                                        return Ok(());
                                    }
                                    _ => {}
                                },
                                Screen::CompareSelection { logs, selected, state } => match key.code {
                                    KeyCode::Up => {
                                        let i = match state.selected() {
                                            Some(i) => {
                                                if i == 0 {
                                                    logs.len() - 1
                                                } else {
                                                    i - 1
                                                }
                                            }
                                            None => 0,
                                        };
                                        state.select(Some(i));
                                    }
                                    KeyCode::Down => {
                                        let i = match state.selected() {
                                            Some(i) => {
                                                if i >= logs.len() - 1 {
                                                    0
                                                } else {
                                                    i + 1
                                                }
                                            }
                                            None => 0,
                                        };
                                        state.select(Some(i));
                                    }
                                    KeyCode::Char(' ') => {
                                        if let Some(i) = state.selected() {
                                            selected[i] = !selected[i];
                                        }
                                    }
                                    KeyCode::Enter => {
                                        let selected_paths: Vec<PathBuf> = logs.iter().enumerate()
                                            .filter(|(i, _)| selected[*i])
                                            .map(|(_, p)| p.clone())
                                            .collect();
                                        if selected_paths.len() < 2 {
                                            app.status_message = "Please select at least 2 logs to compare!".to_string();
                                        } else {
                                            app.status_message.clear();
                                            app.screen = Screen::CompareInputFunction {
                                                selected_logs: selected_paths,
                                            };
                                            app.input_mode = InputMode::Editing;
                                            app.input_value.clear();
                                        }
                                    }
                                    KeyCode::Esc | KeyCode::Char('b') | KeyCode::Char('B') => {
                                        app.screen = Screen::MainMenu;
                                        app.status_message.clear();
                                    }
                                    _ => {}
                                },
                                Screen::CompareResult { selected_logs, function_name, .. } => match key.code {
                                    KeyCode::Char('s') | KeyCode::Char('S') => {
                                        let logs_dir = app.current_dir.join("logs");
                                        let mut logs_meta = Vec::new();
                                        let mut err_msg = None;
                                        for path in selected_logs.iter() {
                                            let label = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Log").to_string();
                                            match load_log_target(&path.to_string_lossy(), label, function_name.as_deref()) {
                                                Ok(log) => logs_meta.push(log),
                                                Err(e) => {
                                                    err_msg = Some(format!("Error loading log: {}", e));
                                                    break;
                                                }
                                            }
                                        }
                                        if let Some(msg) = err_msg {
                                            app.status_message = msg;
                                        } else {
                                            let metadata = CompareMetadata {
                                                command: "compare".to_string(),
                                                timestamp: get_fs_safe_timestamp(),
                                                input_files: selected_logs.iter().map(|p| p.to_string_lossy().to_string()).collect(),
                                                tool_version: env!("CARGO_PKG_VERSION").to_string(),
                                                prompt: Some("TUI comparison".to_string()),
                                            };
                                            let active_labels: Vec<String> = logs_meta.iter().map(|l| l.label.clone()).collect();
                                            let values: Vec<f64> = if logs_meta.iter().all(|l| l.is_code_analyzer_log) {
                                                logs_meta.iter().map(|l| l.code_total_lines.unwrap_or(0) as f64).collect()
                                            } else {
                                                logs_meta.iter().map(|l| l.error_count as f64).collect()
                                            };
                                            let title = if logs_meta.iter().all(|l| l.is_code_analyzer_log) { "Total Source Code Lines" } else { "Error Count" };
                                            let graph_data = GraphDataJson {
                                                title: title.to_string(),
                                                labels: active_labels,
                                                values,
                                            };
                                            let mut table_data = Vec::new();
                                            table_data.push(vec!["Label".to_string()].into_iter().chain(logs_meta.iter().map(|l| l.label.clone())).collect());
                                            table_data.push(vec!["Source".to_string()].into_iter().chain(logs_meta.iter().map(|l| l.source.clone())).collect());
                                            table_data.push(vec!["Total Lines".to_string()].into_iter().chain(logs_meta.iter().map(|l| l.total_lines.to_string())).collect());
                                            table_data.push(vec!["Error Count".to_string()].into_iter().chain(logs_meta.iter().map(|l| l.error_count.to_string())).collect());
                                            table_data.push(vec!["Warn Count".to_string()].into_iter().chain(logs_meta.iter().map(|l| l.warn_count.to_string())).collect());

                                            match save_compare_result_json(metadata, logs_meta, "General comparison summary".to_string(), table_data, graph_data, Some(&logs_dir.to_string_lossy())) {
                                                Ok(saved_path) => app.status_message = format!("Saved comparison result to: {}", saved_path),
                                                Err(e) => app.status_message = format!("Error saving comparison result: {}", e),
                                            }
                                        }
                                    }
                                    KeyCode::Char('b') | KeyCode::Char('B') | KeyCode::Esc => {
                                        app.screen = Screen::MainMenu;
                                        app.status_message.clear();
                                    }
                                    KeyCode::Char('q') | KeyCode::Char('Q') => {
                                        return Ok(());
                                    }
                                    _ => {}
                                },
                                Screen::ListLogs { logs, state, viewing_log, confirm_delete } => {
                                    if confirm_delete.is_some() {
                                        match key.code {
                                            KeyCode::Char('y') | KeyCode::Char('Y') => {
                                                let to_delete = confirm_delete.clone().unwrap();
                                                let _ = std::fs::remove_file(&to_delete);
                                                app.status_message = format!("Deleted {}", to_delete.file_name().unwrap().to_string_lossy());
                                                *confirm_delete = None;
                                                let logs_dir = app.current_dir.join("logs");
                                                *logs = list_log_files(&logs_dir);
                                                let len = logs.len();
                                                let curr = state.selected().unwrap_or(0);
                                                if len == 0 {
                                                    state.select(None);
                                                } else if curr >= len {
                                                    state.select(Some(len - 1));
                                                } else {
                                                    state.select(Some(curr));
                                                }
                                            }
                                            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                                *confirm_delete = None;
                                                app.status_message = "Deletion cancelled.".to_string();
                                            }
                                            _ => {}
                                        }
                                    } else if viewing_log.is_some() {
                                        match key.code {
                                            KeyCode::Esc | KeyCode::Char('b') | KeyCode::Char('B') => {
                                                *viewing_log = None;
                                            }
                                            _ => {}
                                        }
                                    } else {
                                        match key.code {
                                            KeyCode::Up => {
                                                let i = match state.selected() {
                                                    Some(i) => {
                                                        if i == 0 {
                                                            logs.len() - 1
                                                        } else {
                                                            i - 1
                                                        }
                                                    }
                                                    None => 0,
                                                };
                                                state.select(Some(i));
                                            }
                                            KeyCode::Down => {
                                                let i = match state.selected() {
                                                    Some(i) => {
                                                        if i >= logs.len() - 1 {
                                                            0
                                                        } else {
                                                            i + 1
                                                        }
                                                    }
                                                    None => 0,
                                                };
                                                state.select(Some(i));
                                            }
                                            KeyCode::Enter => {
                                                if let Some(i) = state.selected() {
                                                    if let Ok(content) = std::fs::read_to_string(&logs[i]) {
                                                        *viewing_log = Some(content);
                                                    }
                                                }
                                            }
                                            KeyCode::Char('d') | KeyCode::Char('D') => {
                                                if let Some(i) = state.selected() {
                                                    *confirm_delete = Some(logs[i].clone());
                                                }
                                            }
                                            KeyCode::Esc | KeyCode::Char('b') | KeyCode::Char('B') => {
                                                app.screen = Screen::MainMenu;
                                                app.status_message.clear();
                                            }
                                            _ => {}
                                        }
                                    }
                                }
                                Screen::Help => match key.code {
                                    KeyCode::Esc | KeyCode::Char('b') | KeyCode::Char('B') | KeyCode::Enter => {
                                        app.screen = Screen::MainMenu;
                                    }
                                    _ => {}
                                },
                                _ => {}
                            }
                        }
                        InputMode::Editing => {
                            match key.code {
                                KeyCode::Enter => {
                                    let input = app.input_value.trim().to_string();
                                    app.input_mode = InputMode::Normal;
                                    match &mut app.screen {
                                        Screen::AnalyzeInputFiles { selected_files } => {
                                            if input.is_empty() {
                                                if selected_files.is_empty() {
                                                    app.screen = Screen::MainMenu;
                                                } else {
                                                    let files = selected_files.clone();
                                                    app.screen = Screen::AnalyzeInputFunction { files };
                                                    app.input_mode = InputMode::Editing;
                                                    app.input_value.clear();
                                                }
                                            } else if input.to_lowercase() == "done" || input.to_lowercase() == "d" {
                                                if selected_files.is_empty() {
                                                    app.status_message = "No files selected. Add at least one file or press Esc.".to_string();
                                                    app.input_mode = InputMode::Editing;
                                                } else {
                                                    let files = selected_files.clone();
                                                    app.screen = Screen::AnalyzeInputFunction { files };
                                                    app.input_mode = InputMode::Editing;
                                                    app.input_value.clear();
                                                }
                                            } else {
                                                // Check if it's a number selecting a matching suggestion
                                                let matching = scan_analyzable_files(&app.current_dir, &app.current_dir);
                                                let filtered: Vec<String> = matching.iter()
                                                    .filter(|f| f.to_lowercase().contains(&input.to_lowercase()))
                                                    .cloned()
                                                    .collect();
                                                if let Ok(idx) = input.parse::<usize>() {
                                                    if idx > 0 && idx <= filtered.len() {
                                                        let file_to_add = &filtered[idx - 1];
                                                        match resolve_path(file_to_add, &app.current_dir) {
                                                            Ok(p) => {
                                                                let p_str = p.to_string_lossy().to_string();
                                                                if !selected_files.contains(&p_str) {
                                                                    selected_files.push(p_str);
                                                                }
                                                                app.input_value.clear();
                                                                app.input_mode = InputMode::Editing;
                                                                app.status_message = "File added.".to_string();
                                                            }
                                                            Err(e) => {
                                                                app.status_message = e;
                                                                app.input_mode = InputMode::Editing;
                                                            }
                                                        }
                                                    } else {
                                                        match resolve_path(&input, &app.current_dir) {
                                                            Ok(p) => {
                                                                let p_str = p.to_string_lossy().to_string();
                                                                if !selected_files.contains(&p_str) {
                                                                    selected_files.push(p_str);
                                                                }
                                                                app.input_value.clear();
                                                                app.input_mode = InputMode::Editing;
                                                                app.status_message = "File added.".to_string();
                                                            }
                                                            Err(e) => {
                                                                app.status_message = e;
                                                                app.input_mode = InputMode::Editing;
                                                            }
                                                        }
                                                    }
                                                } else {
                                                    match resolve_path(&input, &app.current_dir) {
                                                        Ok(p) => {
                                                            let p_str = p.to_string_lossy().to_string();
                                                            if !selected_files.contains(&p_str) {
                                                                selected_files.push(p_str);
                                                            }
                                                            app.input_value.clear();
                                                            app.input_mode = InputMode::Editing;
                                                            app.status_message = "File added.".to_string();
                                                        }
                                                        Err(e) => {
                                                            app.status_message = e;
                                                            app.input_mode = InputMode::Editing;
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                        Screen::AnalyzeInputFunction { files } => {
                                            let term = input.clone();
                                            let functions = extract_functions_from_files(files);
                                            let filtered: Vec<(String, String)> = functions.iter()
                                                .filter(|(name, _)| name.to_lowercase().contains(&term.to_lowercase()))
                                                .cloned()
                                                .collect();

                                            let selected_func = if term.is_empty() {
                                                None
                                            } else if let Ok(idx) = term.parse::<usize>() {
                                                if idx > 0 && idx <= filtered.len() {
                                                    Some(filtered[idx - 1].0.clone())
                                                } else {
                                                    Some(term)
                                                }
                                            } else {
                                                // Find exact or partial matching functions
                                                let exact_matches: Vec<(String, String)> = filtered.iter()
                                                    .filter(|(name, _)| name.to_lowercase() == term.to_lowercase())
                                                    .cloned()
                                                    .collect();
                                                if exact_matches.len() == 1 {
                                                    Some(exact_matches[0].0.clone())
                                                } else if exact_matches.len() > 1 {
                                                    // Ambiguous function with same name in multiple files!
                                                    // Let's see if the term matches one of them exactly including file or if we should complain.
                                                    // The user typed the exact name, but it matches multiple files.
                                                    // Since it's ambiguous, let the system apply apply_function_scope which will throw the EInvalidInput/Ambiguity error, or we can format it nicely.
                                                    Some(exact_matches[0].0.clone())
                                                } else if filtered.len() == 1 {
                                                    Some(filtered[0].0.clone())
                                                } else {
                                                    Some(term)
                                                }
                                            };

                                            let mut results = Vec::new();
                                            let mut err_msg = None;
                                            for f in files {
                                                match analyze_file(f) {
                                                    Ok(mut res) => {
                                                        if let Some(ref fn_name) = selected_func {
                                                            // If we have multiple files and the function is not in this specific file, apply_function_scope might fail.
                                                            // We should check if the function exists in this file first if there are multiple files, or let apply_function_scope handle it.
                                                            // The requirement says: "In multi-file analysis, clearly identify ambiguous function names by including their file path and require the user to choose the intended function."
                                                            // If the function name exists in multiple files, apply_function_scope on the file where it exists is correct. But if we analyze multiple files,
                                                            // and only one file has it, applying it to other files will fail with NotFound.
                                                            // Let's only apply the function scope to files that actually contain the function.
                                                            let has_fn = res.functions.iter().any(|func| func.name == *fn_name);
                                                            if has_fn {
                                                                if let Err(e) = apply_function_scope(&mut res, fn_name) {
                                                                    err_msg = Some(format!("Error: {}", e));
                                                                    break;
                                                                }
                                                                results.push(res);
                                                            }
                                                        } else {
                                                            results.push(res);
                                                        }
                                                    }
                                                    Err(e) => {
                                                        err_msg = Some(format!("Could not read file '{}'. Details: {}", f, e));
                                                        break;
                                                    }
                                                }
                                            }

                                            // If no file contained the function and a function was selected:
                                            if err_msg.is_none() && selected_func.is_some() && results.is_empty() {
                                                err_msg = Some(format!("Function '{}' not found in any of the selected files.", selected_func.as_ref().unwrap()));
                                            }

                                            // Let's also check for ambiguity across files. If the user selected a function name that appears in multiple selected files:
                                            if err_msg.is_none() && selected_func.is_some() {
                                                let fn_name = selected_func.as_ref().unwrap();
                                                let files_containing: Vec<String> = functions.iter()
                                                    .filter(|(name, _)| name == fn_name)
                                                    .map(|(_, f)| f.clone())
                                                    .collect();
                                                if files_containing.len() > 1 {
                                                    err_msg = Some(format!(
                                                        "Ambiguity error: Function '{}' exists in multiple files: {}. Please analyze one file at a time to scope to this function.",
                                                        fn_name,
                                                        files_containing.join(", ")
                                                    ));
                                                }
                                            }

                                            app.screen = Screen::AnalyzeResult { results, error: err_msg };
                                        }
                                        Screen::CompareInputFunction { selected_logs } => {
                                            let func_name = if input.is_empty() { None } else { Some(input.clone()) };
                                            
                                            let mut logs_meta = Vec::new();
                                            let mut err_msg = None;
                                            for path in selected_logs.iter() {
                                                let label = path.file_stem().and_then(|s| s.to_str()).unwrap_or("Log").to_string();
                                                match load_log_target(&path.to_string_lossy(), label, func_name.as_deref()) {
                                                    Ok(log) => logs_meta.push(log),
                                                    Err(e) => {
                                                        err_msg = Some(format!("Error loading log: {}", e));
                                                        break;
                                                    }
                                                }
                                            }

                                            let mut comparison_text = String::new();
                                            if err_msg.is_none() {
                                                if selected_logs.len() == 2 && logs_meta[0].is_code_analyzer_log && logs_meta[1].is_code_analyzer_log {
                                                    let mut old = parse_analysis_result(&selected_logs[0].to_string_lossy()).unwrap();
                                                    let mut new = parse_analysis_result(&selected_logs[1].to_string_lossy()).unwrap();
                                                    if let Some(ref fn_name) = func_name {
                                                        let _ = apply_function_scope(&mut old, fn_name);
                                                        let _ = apply_function_scope(&mut new, fn_name);
                                                    }
                                                    comparison_text.push_str(&format!("Comparison: {} vs {}\n\n", selected_logs[0].file_name().unwrap().to_string_lossy(), selected_logs[1].file_name().unwrap().to_string_lossy()));
                                                    comparison_text.push_str(&format!("Code Lines: {} vs {}\n", old.total_lines, new.total_lines));
                                                    comparison_text.push_str(&format!("Non-empty:  {} vs {}\n", old.non_empty_lines, new.non_empty_lines));
                                                    comparison_text.push_str(&format!("Imports:    {} vs {}\n", old.import_lines, new.import_lines));
                                                    comparison_text.push_str(&format!("Fns defined:{} vs {}\n", old.function_definitions, new.function_definitions));
                                                } else {
                                                    comparison_text.push_str("General comparison of logs:\n\n");
                                                    for log in &logs_meta {
                                                        if log.is_code_analyzer_log {
                                                            comparison_text.push_str(&format!("  - {} (Code Log): {} lines, {} fns\n", log.label, log.code_total_lines.unwrap_or(0), log.code_function_definitions.unwrap_or(0)));
                                                        } else {
                                                            comparison_text.push_str(&format!("  - {}: {} lines, {} errs, {} warns\n", log.label, log.total_lines, log.error_count, log.warn_count));
                                                        }
                                                    }
                                                }
                                            }

                                            app.screen = Screen::CompareResult {
                                                selected_logs: selected_logs.clone(),
                                                function_name: func_name,
                                                comparison_text,
                                                error: err_msg,
                                            };
                                        }
                                        Screen::ChangeFolder => {
                                            let new_path = PathBuf::from(input);
                                            if new_path.is_dir() {
                                                app.current_dir = new_path;
                                                app.status_message = format!("Folder changed to: {}", app.current_dir.display());
                                                app.screen = Screen::MainMenu;
                                            } else {
                                                app.status_message = "Path does not exist or is not a directory!".to_string();
                                                app.screen = Screen::MainMenu;
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                KeyCode::Tab => {
                                    match &app.screen {
                                        Screen::AnalyzeInputFiles { .. } => {
                                            let matching = scan_analyzable_files(&app.current_dir, &app.current_dir);
                                            let term = app.input_value.trim().to_lowercase();
                                            let filtered: Vec<String> = matching.iter()
                                                .filter(|f| f.to_lowercase().contains(&term))
                                                .cloned()
                                                .collect();
                                            if filtered.len() == 1 {
                                                if let Ok(p) = resolve_path(&filtered[0], &app.current_dir) {
                                                    app.input_value = p.to_string_lossy().to_string();
                                                } else {
                                                    app.input_value = filtered[0].clone();
                                                }
                                            } else if !filtered.is_empty() {
                                                let common = longest_common_prefix(&filtered);
                                                if common.len() > app.input_value.len() {
                                                    app.input_value = common;
                                                }
                                            }
                                        }
                                        Screen::AnalyzeInputFunction { files } => {
                                            let functions = extract_functions_from_files(files);
                                            let term = app.input_value.trim().to_lowercase();
                                            let filtered: Vec<String> = functions.iter()
                                                .map(|(name, _)| name.clone())
                                                .filter(|name| name.to_lowercase().contains(&term))
                                                .collect();
                                            // Deduplicate
                                            let mut unique = filtered.clone();
                                            unique.sort();
                                            unique.dedup();
                                            if unique.len() == 1 {
                                                app.input_value = unique[0].clone();
                                            } else if !unique.is_empty() {
                                                let common = longest_common_prefix(&unique);
                                                if common.len() > app.input_value.len() {
                                                    app.input_value = common;
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                                KeyCode::Esc => {
                                    app.input_mode = InputMode::Normal;
                                    app.screen = Screen::MainMenu;
                                    app.status_message.clear();
                                }
                                KeyCode::Char(c) => {
                                    app.input_value.push(c);
                                }
                                KeyCode::Backspace => {
                                    app.input_value.pop();
                                }
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
    }
}

fn list_log_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() && path.extension().and_then(|s| s.to_str()) == Some("log") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn ui(f: &mut ratatui::Frame, app: &mut AppState) {
    let size = f.size();
    
    let main_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(5),    // Body
            Constraint::Length(3), // Footer / Status
        ])
        .split(size);

    // Header
    let header_text = format!("Code Analyzer v2  |  Current Folder: {}", app.current_dir.display());
    let header = Paragraph::new(Line::from(vec![
        Span::styled(header_text, Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan))
    ]))
    .block(Block::default().borders(Borders::BOTTOM));
    f.render_widget(header, main_layout[0]);

    // Footer/Status
    let footer_text = if !app.status_message.is_empty() {
        if app.status_message.contains('\n') {
            Span::styled("Error occurred. See details above in the main area.", Style::default().fg(Color::Yellow))
        } else {
            Span::styled(&app.status_message, Style::default().fg(Color::Yellow))
        }
    } else {
        Span::raw("Keyboard driven terminal menu. Use shortcuts to navigate.")
    };
    let footer = Paragraph::new(Line::from(vec![footer_text]))
        .block(Block::default().borders(Borders::TOP));
    f.render_widget(footer, main_layout[2]);

    // Body based on Screen state
    match &mut app.screen {
        Screen::MainMenu => {
            let menu_items = vec![
                ListItem::new("[A] Analyze source file(s)"),
                ListItem::new("[C] Compare saved log files"),
                ListItem::new("[L] List or delete logs"),
                ListItem::new("[D] Change current folder"),
                ListItem::new("[H] Help"),
                ListItem::new("[Q] Quit"),
            ];
            let list = List::new(menu_items)
                .block(Block::default().title("Main Menu").borders(Borders::ALL));
            f.render_widget(list, main_layout[1]);
        }
        Screen::AnalyzeInputFiles { selected_files } => {
            let matching = scan_analyzable_files(&app.current_dir, &app.current_dir);
            let input = app.input_value.trim();
            let filtered: Vec<String> = matching.iter()
                .filter(|f| f.to_lowercase().contains(&input.to_lowercase()))
                .cloned()
                .collect();

            let mut lines = Vec::new();
            let joined_selected = selected_files.join(", ");
            lines.push(Line::from(vec![
                Span::styled("Selected file(s): ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(if selected_files.is_empty() { "None".to_string() } else { joined_selected }),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(vec![
                Span::styled("Enter source file path (or autocomplete search term):", Style::default().add_modifier(Modifier::BOLD)),
            ]));
            lines.push(Line::from(format!("> {}", app.input_value)));
            lines.push(Line::from(""));

            if !input.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("Matching files in the current folder:", Style::default().add_modifier(Modifier::UNDERLINED | Modifier::BOLD).fg(Color::Cyan)),
                ]));
                if filtered.is_empty() {
                    lines.push(Line::from("No files match the search term."));
                } else {
                    for (idx, path) in filtered.iter().enumerate() {
                        lines.push(Line::from(format!("{}. {}", idx + 1, path)));
                    }
                }
                lines.push(Line::from(""));
            }

            lines.push(Line::from(vec![
                Span::styled("Select a number, continue typing a path, type 'done' to analyze, or press Esc / B to go back.", Style::default().fg(Color::Yellow)),
            ]));

            if !app.status_message.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(vec![
                    Span::styled("Error details:", Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD)),
                ]));
                for line in app.status_message.lines() {
                    lines.push(Line::from(line.to_string()));
                }
            }

            let para = Paragraph::new(lines)
                .block(Block::default().title("Analyze Source File(s) - Interactive Picker").borders(Borders::ALL))
                .wrap(Wrap { trim: true });
            f.render_widget(para, main_layout[1]);
        }
        Screen::AnalyzeInputFunction { files } => {
            let functions = extract_functions_from_files(files);
            let term = app.input_value.trim();
            let filtered: Vec<(String, String)> = functions.iter()
                .filter(|(name, _)| name.to_lowercase().contains(&term.to_lowercase()))
                .cloned()
                .collect();

            let mut lines = Vec::new();
            lines.push(Line::from(vec![
                Span::styled("Analyze a specific function? Leave blank for full file:", Style::default().add_modifier(Modifier::BOLD)),
            ]));
            lines.push(Line::from(format!("> {}", app.input_value)));
            lines.push(Line::from(""));

            if !term.is_empty() {
                lines.push(Line::from(vec![
                    Span::styled("Matching functions:", Style::default().add_modifier(Modifier::UNDERLINED | Modifier::BOLD).fg(Color::Cyan)),
                ]));
                if filtered.is_empty() {
                    lines.push(Line::from("No functions match the search term."));
                } else {
                    for (idx, (name, file_path)) in filtered.iter().enumerate() {
                        let label = if files.len() > 1 {
                            format!("{}. {} (in {})", idx + 1, name, file_path)
                        } else {
                            format!("{}. {}", idx + 1, name)
                        };
                        lines.push(Line::from(label));
                    }
                }
                lines.push(Line::from(""));
            }

            lines.push(Line::from(vec![
                Span::styled("Select a number, continue typing, press [Tab] to autocomplete, or press [Enter] to run.", Style::default().fg(Color::Yellow)),
            ]));

            let para = Paragraph::new(lines)
                .block(Block::default().title("Function Scope Picker").borders(Borders::ALL))
                .wrap(Wrap { trim: true });
            f.render_widget(para, main_layout[1]);
        }
        Screen::AnalyzeResult { results, error } => {
            if let Some(err) = error {
                let err_para = Paragraph::new(err.as_str())
                    .block(Block::default().title("Analysis Error").borders(Borders::ALL))
                    .style(Style::default().fg(Color::Red));
                f.render_widget(err_para, main_layout[1]);
            } else {
                let mut text = Vec::new();
                for r in results {
                    text.push(Line::from(vec![
                        Span::styled(format!("File: {}", r.source_file), Style::default().add_modifier(Modifier::BOLD).fg(Color::Green))
                    ]));
                    text.push(Line::from(format!("Language:             {}", r.language)));
                    text.push(Line::from(format!("Total lines:          {}", r.total_lines)));
                    text.push(Line::from(format!("Non-empty lines:      {}", r.non_empty_lines)));
                    text.push(Line::from(format!("Import lines:         {}", r.import_lines)));
                    text.push(Line::from(format!("Function definitions: {}", r.function_definitions)));
                    if let Some(ref scope) = r.scope {
                        text.push(Line::from(format!("Scoped to function:   {}", scope.name)));
                    }
                    if !r.functions.is_empty() {
                        text.push(Line::from("Functions found:"));
                        for func in &r.functions {
                            text.push(Line::from(format!("  - {} (lines {}-{}, total: {})", func.name, func.start_line, func.end_line, func.total_lines)));
                        }
                    }
                    text.push(Line::from(""));
                }
                text.push(Line::from(vec![
                    Span::styled("[S] Save log | [B] Back to Main Menu | [Q] Quit", Style::default().fg(Color::Yellow))
                ]));
                let para = Paragraph::new(text)
                    .block(Block::default().title("Analysis Summary").borders(Borders::ALL))
                    .wrap(Wrap { trim: true });
                f.render_widget(para, main_layout[1]);
            }
        }
        Screen::CompareSelection { logs, selected, state } => {
            let items: Vec<ListItem> = logs.iter().enumerate()
                .map(|(i, path)| {
                    let checkbox = if selected[i] { "[x] " } else { "[ ] " };
                    let filename = path.file_name().unwrap_or_default().to_string_lossy();
                    ListItem::new(format!("{}{}", checkbox, filename))
                })
                .collect();
            let list = List::new(items)
                .block(Block::default().title("Select logs to compare (Space to select/deselect, Enter to compare, Esc to go back):").borders(Borders::ALL))
                .highlight_symbol(">> ");
            f.render_stateful_widget(list, main_layout[1], state);
        }
        Screen::CompareInputFunction { .. } => {
            let edit_block = Paragraph::new(app.input_value.as_str())
                .block(Block::default().title("Enter function name for scoping comparison (leave blank for all):").borders(Borders::ALL));
            f.render_widget(edit_block, main_layout[1]);
        }
        Screen::CompareResult { comparison_text, error, .. } => {
            if let Some(err) = error {
                let err_para = Paragraph::new(err.as_str())
                    .block(Block::default().title("Comparison Error").borders(Borders::ALL))
                    .style(Style::default().fg(Color::Red));
                f.render_widget(err_para, main_layout[1]);
            } else {
                let mut text = Vec::new();
                for line in comparison_text.lines() {
                    text.push(Line::from(line));
                }
                text.push(Line::from(""));
                text.push(Line::from(vec![
                    Span::styled("[S] Save Comparison | [B] Back to Main Menu | [Q] Quit", Style::default().fg(Color::Yellow))
                ]));
                let para = Paragraph::new(text)
                    .block(Block::default().title("Comparison Summary & Results").borders(Borders::ALL))
                    .wrap(Wrap { trim: true });
                f.render_widget(para, main_layout[1]);
            }
        }
        Screen::ListLogs { logs, state, viewing_log, confirm_delete } => {
            if let Some(to_del) = confirm_delete {
                let text = format!("Delete {}? [y/N]", to_del.file_name().unwrap().to_string_lossy());
                let modal_area = centered_rect(60, 20, size);
                let clear = Clear;
                let block = Paragraph::new(text)
                    .block(Block::default().title("Confirm Deletion").borders(Borders::ALL))
                    .style(Style::default().fg(Color::Red));
                f.render_widget(clear, modal_area);
                f.render_widget(block, modal_area);
            } else if let Some(content) = viewing_log {
                let mut text = Vec::new();
                for line in content.lines() {
                    text.push(Line::from(line));
                }
                text.push(Line::from(""));
                text.push(Line::from(vec![
                    Span::styled("[B] Back to Logs list", Style::default().fg(Color::Yellow))
                ]));
                let para = Paragraph::new(text)
                    .block(Block::default().title("Viewing Saved Log File").borders(Borders::ALL))
                    .wrap(Wrap { trim: true });
                f.render_widget(para, main_layout[1]);
            } else if logs.is_empty() {
                let no_logs = Paragraph::new("No log files found in logs/ directory.")
                    .block(Block::default().title("Saved Logs").borders(Borders::ALL));
                f.render_widget(no_logs, main_layout[1]);
            } else {
                let items: Vec<ListItem> = logs.iter()
                    .map(|p| ListItem::new(p.file_name().unwrap().to_string_lossy().to_string()))
                    .collect();
                let list = List::new(items)
                    .block(Block::default().title("Saved Log Files (Enter to view, D to delete, Esc to go back):").borders(Borders::ALL))
                    .highlight_symbol(">> ");
                f.render_stateful_widget(list, main_layout[1], state);
            }
        }
        Screen::ChangeFolder => {
            let edit_block = Paragraph::new(app.input_value.as_str())
                .block(Block::default().title("Enter new folder path:").borders(Borders::ALL));
            f.render_widget(edit_block, main_layout[1]);
        }
        Screen::Help => {
            let text = vec![
                Line::from(vec![Span::styled("Code Analyzer Keyboard Shortcuts Guide", Style::default().add_modifier(Modifier::BOLD))]),
                Line::from(""),
                Line::from("A         Analyze source file(s)"),
                Line::from("C         Compare saved log files"),
                Line::from("L         List or delete saved logs"),
                Line::from("D         Change current working folder"),
                Line::from("H         Show this shortcuts guide"),
                Line::from("Q / Esc   Quit or return to previous screen"),
                Line::from(""),
                Line::from("Press Enter or B to go back to the Main Menu."),
            ];
            let para = Paragraph::new(text)
                .block(Block::default().title("Keyboard Shortcuts").borders(Borders::ALL));
            f.render_widget(para, main_layout[1]);
        }
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_function_definitions() {
        assert!(is_rust_function_definition("fn hello() {}"));
        assert!(is_rust_function_definition("   fn hello() {}"));
        assert!(is_rust_function_definition("fn_something_else") == false);
        assert!(is_rust_function_definition("fn()"));
        assert!(!is_rust_function_definition("pub fn hello()"));
        assert!(!is_rust_function_definition("let x = fn_name();"));
        assert!(!is_rust_function_definition("fn_call();"));
        assert!(!is_rust_function_definition("some other text"));
    }

    #[test]
    fn test_javascript_function_definitions() {
        assert!(is_javascript_function_definition("function add(a, b) {"));
        assert!(is_javascript_function_definition("   function add(a, b) {"));
        assert!(is_javascript_function_definition("async function fetchData(url) {"));
        assert!(is_javascript_function_definition("  async function fetchData(url) {"));
        assert!(is_javascript_function_definition("const multiply = (a, b) => a * b;"));
        assert!(is_javascript_function_definition("let greet = name => `Hello, ${name}!`;"));
        assert!(is_javascript_function_definition("var power = (base, exp) => { return Math.pow(base, exp); };"));
        assert!(is_javascript_function_definition("   const asyncArrow = async () => {};"));
        assert!(is_javascript_function_definition("export function hello() {}"));
        assert!(is_javascript_function_definition("export default async function hello() {}"));
        assert!(is_javascript_function_definition("export const hello = () => {}"));
        assert!(!is_javascript_function_definition("const result = multiply(add(2, 3), 4);"));
        assert!(!is_javascript_function_definition("console.log(result);"));
        assert!(!is_javascript_function_definition("const x = 5; // value => 6"));
        assert!(!is_javascript_function_definition("let y = \"this is not an arrow => function\";"));
        assert!(!is_javascript_function_definition("var a = b >= c;"));
    }

    #[test]
    fn test_analysis_logging() {
        let mut log_path = std::env::temp_dir();
        log_path.push(format!("test_log_{}.log", Utc::now().timestamp_millis()));
        let log_file_str = log_path.to_str().unwrap();

        let _ = std::fs::remove_file(&log_path);

        let result1 = AnalysisResult {
            source_file: "test1.rs".to_string(),
            language: "Rust".to_string(),
            analyzed_at: "2026-08-05T00:15:00Z".to_string(),
            total_lines: 10,
            non_empty_lines: 8,
            import_lines: 2,
            scope: None,
            function_definitions: 1,
            functions: vec![FunctionResult {
                name: "test_fn".to_string(),
                start_line: 4,
                end_line: 8,
                total_lines: 5,
                non_empty_lines: 4,
            }],
        };

        save_analysis_result(&result1, log_file_str).unwrap();
        assert!(log_path.exists());

        let contents1 = std::fs::read_to_string(&log_path).unwrap();
        assert!(contents1.contains("source_file=test1.rs"));
        assert!(contents1.contains("language=Rust"));
        assert!(contents1.contains("analyzed_at=2026-08-05T00:15:00Z"));
        assert!(contents1.contains("total_lines=10"));
        assert!(contents1.contains("non_empty_lines=8"));
        assert!(contents1.contains("import_lines=2"));
        assert!(contents1.contains("function_definitions=1"));
        assert!(contents1.contains("function=test_fn:4:8:5:4"));

        let result2 = AnalysisResult {
            source_file: "test2.js".to_string(),
            language: "JavaScript".to_string(),
            analyzed_at: "2026-08-05T00:16:00Z".to_string(),
            total_lines: 20,
            non_empty_lines: 15,
            import_lines: 3,
            scope: None,
            function_definitions: 2,
            functions: vec![],
        };

        save_analysis_result(&result2, log_file_str).unwrap();

        let contents2 = std::fs::read_to_string(&log_path).unwrap();
        assert!(contents2.contains("source_file=test1.rs"));
        assert!(contents2.contains("source_file=test2.js"));

        let parsed = parse_analysis_result(log_file_str).unwrap();
        assert_eq!(parsed.source_file, "test2.js");
        assert_eq!(parsed.language, "JavaScript");
        assert_eq!(parsed.analyzed_at, "2026-08-05T00:16:00Z");
        assert_eq!(parsed.total_lines, 20);
        assert_eq!(parsed.non_empty_lines, 15);
        assert_eq!(parsed.import_lines, 3);
        assert_eq!(parsed.function_definitions, 2);
        assert!(parsed.functions.is_empty());

        let _ = std::fs::remove_file(&log_path);
    }

    #[test]
    fn test_compare_results_metrics() {
        let mut old_path = std::env::temp_dir();
        old_path.push(format!("old_log_{}.log", Utc::now().timestamp_millis()));
        let old_str = old_path.to_str().unwrap();

        let mut source_path = std::env::temp_dir();
        source_path.push(format!("test_src_{}.rs", Utc::now().timestamp_millis() + 2));
        let source_str = source_path.to_str().unwrap();

        let _ = std::fs::remove_file(&old_path);
        let _ = std::fs::remove_file(&source_path);

        let old_res = AnalysisResult {
            source_file: "old.rs".to_string(),
            language: "Rust".to_string(),
            analyzed_at: "2026-08-05T00:15:00Z".to_string(),
            total_lines: 10,
            non_empty_lines: 8,
            import_lines: 2,
            scope: None,
            function_definitions: 1,
            functions: vec![],
        };

        save_analysis_result(&old_res, old_str).unwrap();

        std::fs::write(&source_path, "use std::io;\nfn hello() {}\nfn world() {}\n").unwrap();

        let res = compare_current_with_log(source_str, old_str, None);
        assert!(res.is_ok());

        let source_log_path = source_path.with_extension("log");
        assert!(!source_log_path.exists());

        let _ = std::fs::remove_file(&old_path);
        let _ = std::fs::remove_file(&source_path);
    }

    #[test]
    fn test_invalid_log_files() {
        let mut path = std::env::temp_dir();
        path.push(format!("invalid_log_{}.log", Utc::now().timestamp_millis()));
        let path_str = path.to_str().unwrap();

        let _ = std::fs::remove_file(&path);
        let err = parse_analysis_result(path_str).unwrap_err();
        assert!(err.to_string().contains("Could not open log file"));

        let write_content = |content: &str| {
            std::fs::write(&path, content).unwrap();
        };

        write_content("source_file: test.rs\nlanguage=Rust\n");
        let err = parse_analysis_result(path_str).unwrap_err();
        assert!(err.to_string().contains("Malformed entry"));

        write_content("source_file=test.rs\nlanguage=Rust\ntotal_lines=10\nnon_empty_lines=8\nimport_lines=2\nfunction_definitions=1\n");
        let err = parse_analysis_result(path_str).unwrap_err();
        assert!(err.to_string().contains("Missing required field 'analyzed_at'"));

        write_content("source_file=test.rs\nlanguage=Rust\nanalyzed_at=2026-08-05T00:15:00Z\ntotal_lines=abc\nnon_empty_lines=8\nimport_lines=2\nfunction_definitions=1\n");
        let err = parse_analysis_result(path_str).unwrap_err();
        assert!(err.to_string().contains("Invalid number for total_lines"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_cli_parsing() {
        let program = "codeanalyzer".to_string();

        let args = vec![program.clone(), "analyze".to_string(), "sample.js".to_string()];
        assert_eq!(
            parse_args(&args),
            Ok(Command::Analyze {
                source_file: "sample.js".to_string(),
                output_file: None,
                function_name: None,
            })
        );

        let args = vec![
            program.clone(),
            "analyze".to_string(),
            "sample.js".to_string(),
            "output.log".to_string(),
        ];
        assert_eq!(
            parse_args(&args),
            Ok(Command::Analyze {
                source_file: "sample.js".to_string(),
                output_file: Some("output.log".to_string()),
                function_name: None,
            })
        );

        let args = vec![
            program.clone(),
            "analyze".to_string(),
            "sample.js".to_string(),
            "--output".to_string(),
            "custom.json".to_string(),
        ];
        assert_eq!(
            parse_args(&args),
            Ok(Command::Analyze {
                source_file: "sample.js".to_string(),
                output_file: Some("custom.json".to_string()),
                function_name: None,
            })
        );

        let args = vec![
            program.clone(),
            "compare".to_string(),
            "sample.js".to_string(),
            "baseline.log".to_string(),
        ];
        assert_eq!(
            parse_args(&args),
            Ok(Command::Compare {
                log_files: vec!["sample.js".to_string(), "baseline.log".to_string()],
                output_file: None,
                function_name: None,
            })
        );

        let args = vec![
            program.clone(),
            "compare".to_string(),
            "log1.log".to_string(),
            "log2.log".to_string(),
            "log3.log".to_string(),
            "--output".to_string(),
            "comparison.json".to_string(),
        ];
        assert_eq!(
            parse_args(&args),
            Ok(Command::Compare {
                log_files: vec!["log1.log".to_string(), "log2.log".to_string(), "log3.log".to_string()],
                output_file: Some("comparison.json".to_string()),
                function_name: None,
            })
        );

        let args = vec![program.clone()];
        assert_eq!(parse_args(&args), Ok(Command::Interactive));

        let args = vec![program.clone(), "invalid_cmd".to_string(), "sample.js".to_string()];
        assert!(parse_args(&args).is_err());

        let args = vec![program.clone(), "analyze".to_string()];
        assert!(parse_args(&args).is_err());

        let args = vec![program.clone(), "compare".to_string(), "sample.js".to_string()];
        assert!(parse_args(&args).is_err());

        let args = vec![
            program.clone(),
            "compare-multi".to_string(),
            "log1.log".to_string(),
            "log2.log".to_string(),
        ];
        assert_eq!(
            parse_args(&args),
            Ok(Command::Compare {
                log_files: vec!["log1.log".to_string(), "log2.log".to_string()],
                output_file: None,
                function_name: None,
            })
        );

        assert_eq!(
            parse_args(&vec![program.clone(), "--help".to_string()]),
            Ok(Command::Help)
        );
        assert_eq!(
            parse_args(&vec![program.clone(), "-h".to_string()]),
            Ok(Command::Help)
        );
        assert_eq!(
            parse_args(&vec![program.clone(), "help".to_string()]),
            Ok(Command::Help)
        );
    }

    #[test]
    fn test_rust_and_js_function_parsing() {
        let mut rust_path = std::env::temp_dir();
        rust_path.push(format!("test_fn_rust_{}.rs", Utc::now().timestamp_millis()));
        let rust_str = rust_path.to_str().unwrap();

        let rust_code = r#"
fn test_one() {
    println!("Hello");
}

fn test_two(x: i32) -> i32 {
    
    x + 1
}
"#;
        std::fs::write(&rust_path, rust_code).unwrap();
        let res = analyze_file(rust_str).unwrap();
        assert_eq!(res.functions.len(), 2);
        assert_eq!(res.functions[0].name, "test_one");
        assert_eq!(res.functions[0].start_line, 2);
        assert_eq!(res.functions[0].end_line, 4);
        assert_eq!(res.functions[0].total_lines, 3);
        assert_eq!(res.functions[0].non_empty_lines, 3);

        assert_eq!(res.functions[1].name, "test_two");
        assert_eq!(res.functions[1].start_line, 6);
        assert_eq!(res.functions[1].end_line, 9);
        assert_eq!(res.functions[1].total_lines, 4);
        assert_eq!(res.functions[1].non_empty_lines, 3);

        let _ = std::fs::remove_file(&rust_path);

        let mut js_path = std::env::temp_dir();
        js_path.push(format!("test_fn_js_{}.js", Utc::now().timestamp_millis()));
        let js_str = js_path.to_str().unwrap();

        let js_code = r#"
function calculate(a) {
    return a * 2;
}
const add = (x, y) => {
    return x + y;
};
"#;
        std::fs::write(&js_path, js_code).unwrap();
        let res = analyze_file(js_str).unwrap();
        assert_eq!(res.functions.len(), 2);
        assert_eq!(res.functions[0].name, "calculate");
        assert_eq!(res.functions[1].name, "add");
        assert_eq!(res.functions[0].total_lines, 3);
        assert_eq!(res.functions[1].total_lines, 3);

        let _ = std::fs::remove_file(&js_path);
    }

    #[test]
    fn test_compatibility_older_logs_missing_functions() {
        let mut log_path = std::env::temp_dir();
        log_path.push(format!("old_format_{}.log", Utc::now().timestamp_millis()));
        let log_str = log_path.to_str().unwrap();

        let old_content = "source_file=test.rs\nlanguage=Rust\nanalyzed_at=2026-08-05T00:15:00Z\ntotal_lines=10\nnon_empty_lines=8\nimport_lines=2\nfunction_definitions=1\n";
        std::fs::write(&log_path, old_content).unwrap();

        let res = parse_analysis_result(log_str);
        assert!(res.is_ok());
        let parsed = res.unwrap();
        assert_eq!(parsed.functions.len(), 0);

        let _ = std::fs::remove_file(&log_path);
    }

    #[test]
    fn test_malformed_function_log_data() {
        let mut log_path = std::env::temp_dir();
        log_path.push(format!("malformed_fn_{}.log", Utc::now().timestamp_millis()));
        let log_str = log_path.to_str().unwrap();

        let content = "source_file=test.rs\nlanguage=Rust\nanalyzed_at=2026-08-05T00:15:00Z\ntotal_lines=10\nnon_empty_lines=8\nimport_lines=2\nfunction_definitions=1\nfunction=bad_format:1\n";
        std::fs::write(&log_path, content).unwrap();

        let res = parse_analysis_result(log_str);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("Malformed function entry"));

        let _ = std::fs::remove_file(&log_path);
    }

    #[test]
    fn test_three_plus_log_comparison() {
        let log1 = "source_file=test1.rs\nlanguage=Rust\nanalyzed_at=2026-08-05T00:15:00Z\ntotal_lines=10\nnon_empty_lines=8\nimport_lines=2\nfunction_definitions=1\n";
        let log2 = "source_file=test2.rs\nlanguage=Rust\nanalyzed_at=2026-08-05T00:16:00Z\ntotal_lines=20\nnon_empty_lines=15\nimport_lines=3\nfunction_definitions=2\n";
        let log3 = "source_file=test3.rs\nlanguage=Rust\nanalyzed_at=2026-08-05T00:17:00Z\ntotal_lines=30\nnon_empty_lines=22\nimport_lines=4\nfunction_definitions=3\n";

        let meta1 = parse_log_content(log1, "Log 1".to_string(), "file1".to_string());
        let meta2 = parse_log_content(log2, "Log 2".to_string(), "file2".to_string());
        let meta3 = parse_log_content(log3, "Log 3".to_string(), "file3".to_string());

        assert!(meta1.is_code_analyzer_log);
        assert!(meta2.is_code_analyzer_log);
        assert!(meta3.is_code_analyzer_log);

        assert_eq!(meta1.code_total_lines, Some(10));
        assert_eq!(meta2.code_total_lines, Some(20));
        assert_eq!(meta3.code_total_lines, Some(30));
    }

    #[test]
    fn test_chart_data_generation() {
        let logs_content = "2026-08-05T00:15:00Z INFO API Latency: 120ms\n2026-08-05T00:15:01Z ERROR Timeout occurred\n2026-08-05T00:15:02Z INFO API Latency: 150ms\n";
        let meta = parse_log_content(logs_content, "Prod".to_string(), "prod.log".to_string());

        assert_eq!(meta.error_count, 1);
        assert_eq!(meta.info_count, 2);
        assert_eq!(meta.total_lines, 3);
        assert!(meta.avg_latency_ms.is_some());
        assert_eq!(meta.avg_latency_ms, Some(135.0));
        assert_eq!(meta.min_latency_ms, Some(120.0));
        assert_eq!(meta.max_latency_ms, Some(150.0));
    }

    #[test]
    fn test_invalid_empty_logs_multi() {
        let empty_content = "";
        let meta = parse_log_content(empty_content, "Empty".to_string(), "empty.log".to_string());
        assert_eq!(meta.total_lines, 0);
        assert_eq!(meta.error_count, 0);
        assert_eq!(meta.avg_latency_ms, None);
        assert!(!meta.is_code_analyzer_log);

        let invalid_content = "some random unrelated log lines without format";
        let meta2 = parse_log_content(invalid_content, "Invalid".to_string(), "invalid.log".to_string());
        assert_eq!(meta2.total_lines, 1);
        assert_eq!(meta2.error_count, 0);
        assert_eq!(meta2.avg_latency_ms, None);
        assert!(!meta2.is_code_analyzer_log);
    }

    #[test]
    fn test_filename_generation() {
        let name1 = "src/main.rs";
        let name2 = "a/b/c/test-file.js";
        assert_eq!(sanitize_filename(name1), "main.rs");
        assert_eq!(sanitize_filename(name2), "test-file.js");

        let ts = get_fs_safe_timestamp();
        assert_eq!(ts.len(), 20);
        assert!(ts.contains('T'));
        assert!(ts.ends_with('Z'));
        assert!(!ts.contains(':'));
    }

    #[test]
    fn test_output_override() {
        let temp_dir = std::env::temp_dir();
        let custom_out_path = temp_dir.join("override_analysis.json");
        let custom_out_str = custom_out_path.to_str().unwrap();

        let result = AnalysisResult {
            source_file: "dummy.rs".to_string(),
            language: "Rust".to_string(),
            analyzed_at: "2026-08-09T12:00:00Z".to_string(),
            total_lines: 5,
            non_empty_lines: 5,
            import_lines: 0,
            scope: None,
            function_definitions: 0,
            functions: vec![],
        };

        let saved = save_analysis_result_json(&result, Some(custom_out_str)).unwrap();
        assert_eq!(saved, custom_out_str);
        assert!(custom_out_path.exists());

        let content = std::fs::read_to_string(&custom_out_path).unwrap();
        assert!(content.contains("dummy.rs"));
        let _ = std::fs::remove_file(custom_out_path);
    }

    #[test]
    fn test_compare_execution_integration() {
        let temp_dir = std::env::temp_dir();
        let log1_path = temp_dir.join("test_log1.log");
        let log2_path = temp_dir.join("test_log2.log");
        let log3_path = temp_dir.join("test_log3.log");

        std::fs::write(&log1_path, "INFO error 120ms\n").unwrap();
        std::fs::write(&log2_path, "INFO error 150ms\n").unwrap();
        std::fs::write(&log3_path, "INFO error 200ms\n").unwrap();

        let log_paths = vec![
            log1_path.to_str().unwrap().to_string(),
            log2_path.to_str().unwrap().to_string(),
            log3_path.to_str().unwrap().to_string(),
        ];

        let res = execute_compare(&log_paths, None, None);
        assert!(res.is_ok());

        let res_2 = execute_compare(&log_paths[0..2], None, None);
        assert!(res_2.is_ok());

        let _ = std::fs::remove_file(log1_path);
        let _ = std::fs::remove_file(log2_path);
        let _ = std::fs::remove_file(log3_path);
    }

    #[test]
    fn test_default_saving_and_overrides() {
        let temp_dir = std::env::temp_dir();
        let timestamp_test = Utc::now().timestamp_millis();
        
        let result = AnalysisResult {
            source_file: format!("test_save_{}.js", timestamp_test),
            language: "JavaScript".to_string(),
            analyzed_at: "2026-08-09T12:00:00Z".to_string(),
            total_lines: 10,
            non_empty_lines: 10,
            import_lines: 0,
            scope: None,
            function_definitions: 0,
            functions: vec![],
        };

        let saved_json = save_analysis_result_json(&result, None).unwrap();
        let saved_log = save_analysis_result_log(&result, None).unwrap();
        assert!(saved_json.contains("analysis-results/analyze/"));
        assert!(saved_log.contains("logs/"));
        assert!(Path::new(&saved_json).exists());
        assert!(Path::new(&saved_log).exists());
        let _ = std::fs::remove_file(&saved_json);
        let _ = std::fs::remove_file(&saved_log);

        let override_dir = temp_dir.join(format!("override_dir_{}", timestamp_test));
        std::fs::create_dir_all(&override_dir).unwrap();
        let override_dir_str = override_dir.to_str().unwrap();
        let saved_json_dir = save_analysis_result_json(&result, Some(override_dir_str)).unwrap();
        let saved_log_dir = save_analysis_result_log(&result, Some(override_dir_str)).unwrap();
        assert!(saved_json_dir.starts_with(override_dir_str));
        assert!(saved_log_dir.starts_with(override_dir_str));
        assert!(Path::new(&saved_json_dir).exists());
        assert!(Path::new(&saved_log_dir).exists());
        let _ = std::fs::remove_file(&saved_json_dir);
        let _ = std::fs::remove_file(&saved_log_dir);
        let _ = std::fs::remove_dir(&override_dir);

        let custom_json_path = temp_dir.join(format!("custom_{}.json", timestamp_test));
        let custom_json_str = custom_json_path.to_str().unwrap();
        let saved_json_file = save_analysis_result_json(&result, Some(custom_json_str)).unwrap();
        assert_eq!(saved_json_file, custom_json_str);
        assert!(custom_json_path.exists());
        let _ = std::fs::remove_file(custom_json_path);

        let custom_log_path = temp_dir.join(format!("custom_{}.log", timestamp_test));
        let custom_log_str = custom_log_path.to_str().unwrap();
        let saved_log_file = save_analysis_result_log(&result, Some(custom_log_str)).unwrap();
        assert_eq!(saved_log_file, custom_log_str);
        assert!(custom_log_path.exists());
        let _ = std::fs::remove_file(custom_log_path);
    }

    #[test]
    fn test_function_scoped_analysis() {
        let temp_dir = std::env::temp_dir();
        let js_path = temp_dir.join("test_func_scope.js");
        let js_code = "\
function process_payment(a, b) {
    let result = a + b;
    return result;
}

class PaymentProcessor {
    async execute_payment(amount) {
        console.log(amount);
    }
}
";
        std::fs::write(&js_path, js_code).unwrap();
        let js_path_str = js_path.to_str().unwrap();

        let result = analyze_file(js_path_str).unwrap();
        assert_eq!(result.functions.len(), 2);
        assert_eq!(result.functions[0].name, "process_payment");
        assert_eq!(result.functions[1].name, "execute_payment");

        let mut result_scoped = result.clone();
        apply_function_scope(&mut result_scoped, "process_payment").unwrap();
        assert_eq!(result_scoped.functions.len(), 1);
        assert_eq!(result_scoped.function_definitions, 1);
        assert_eq!(result_scoped.functions[0].name, "process_payment");
        assert_eq!(result_scoped.total_lines, 4);
        assert!(result_scoped.scope.is_some());
        let scope = result_scoped.scope.unwrap();
        assert_eq!(scope.name, "process_payment");
        assert_eq!(scope.scope_type, "function");

        let mut result_method = result.clone();
        apply_function_scope(&mut result_method, "execute_payment").unwrap();
        assert_eq!(result_method.functions.len(), 1);
        assert_eq!(result_method.functions[0].name, "execute_payment");
        assert_eq!(result_method.total_lines, 3);

        let mut result_missing = result.clone();
        let err = apply_function_scope(&mut result_missing, "nonexistent");
        assert!(err.is_err());
        assert_eq!(err.unwrap_err().kind(), io::ErrorKind::NotFound);

        let js_code_ambig = "\
fn test_fn() {
}
fn test_fn() {
}
";
        let rust_path = temp_dir.join("test_ambig.rs");
        std::fs::write(&rust_path, js_code_ambig).unwrap();
        let mut res_ambig = analyze_file(rust_path.to_str().unwrap()).unwrap();
        let err_ambig = apply_function_scope(&mut res_ambig, "test_fn");
        assert!(err_ambig.is_err());
        assert_eq!(err_ambig.unwrap_err().kind(), io::ErrorKind::InvalidInput);

        let _ = std::fs::remove_file(js_path);
        let _ = std::fs::remove_file(rust_path);
    }

    #[test]
    fn test_whole_file_analysis_unchanged() {
        let temp_dir = std::env::temp_dir();
        let js_path = temp_dir.join("test_whole.js");
        let js_code = "\
function add(a, b) {
    return a + b;
}
function sub(a, b) {
    return a - b;
}
";
        std::fs::write(&js_path, js_code).unwrap();
        let result = analyze_file(js_path.to_str().unwrap()).unwrap();
        assert_eq!(result.functions.len(), 2);
        assert_eq!(result.total_lines, 6);
        assert_eq!(result.non_empty_lines, 6);
        assert_eq!(result.import_lines, 0);
        assert_eq!(result.function_definitions, 2);
        assert!(result.scope.is_none());

        let _ = std::fs::remove_file(js_path);
    }

    #[test]
    fn test_tui_saving_under_current_folder() {
        let temp_dir = std::env::temp_dir();
        let app_dir = temp_dir.join("test_tui_app");
        std::fs::create_dir_all(&app_dir).unwrap();

        let result = AnalysisResult {
            source_file: "my_test.js".to_string(),
            language: "JavaScript".to_string(),
            analyzed_at: "2026-08-09T12:00:00Z".to_string(),
            total_lines: 5,
            non_empty_lines: 5,
            import_lines: 0,
            scope: None,
            function_definitions: 0,
            functions: vec![],
        };

        let logs_path = app_dir.join("logs");
        let path = save_analysis_result_log(&result, Some(&logs_path.to_string_lossy())).unwrap();
        assert!(path.contains("test_tui_app/logs/"));
        assert!(Path::new(&path).exists());

        let logs = list_log_files(&logs_path);
        assert_eq!(logs.len(), 1);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(logs_path);
        let _ = std::fs::remove_dir(app_dir);
    }

    #[test]
    fn test_scan_analyzable_files() {
        let temp_dir = std::env::temp_dir();
        let app_dir = temp_dir.join("test_scan_app");
        std::fs::create_dir_all(&app_dir).unwrap();

        // Create target/ and logs/ folders that must be ignored
        let target_dir = app_dir.join("target");
        let logs_dir = app_dir.join("logs");
        std::fs::create_dir_all(&target_dir).unwrap();
        std::fs::create_dir_all(&logs_dir).unwrap();

        std::fs::write(target_dir.join("ignored.rs"), "fn ignored() {}").unwrap();
        std::fs::write(logs_dir.join("ignored.log"), "source_file=abc").unwrap();

        // Create valid source files
        let src_dir = app_dir.join("src");
        std::fs::create_dir_all(&src_dir).unwrap();
        std::fs::write(src_dir.join("payment.rs"), "fn payment() {}").unwrap();
        std::fs::write(app_dir.join("sample.js"), "console.log('hi');").unwrap();
        std::fs::write(app_dir.join("unrelated.txt"), "hello world").unwrap(); // unsupported extension

        let files = scan_analyzable_files(&app_dir, &app_dir);
        
        assert_eq!(files.len(), 2);
        assert!(files.contains(&"sample.js".to_string()));
        assert!(files.contains(&"src/payment.rs".to_string()));

        // Clean up
        let _ = std::fs::remove_file(src_dir.join("payment.rs"));
        let _ = std::fs::remove_file(app_dir.join("sample.js"));
        let _ = std::fs::remove_file(app_dir.join("unrelated.txt"));
        let _ = std::fs::remove_file(target_dir.join("ignored.rs"));
        let _ = std::fs::remove_file(logs_dir.join("ignored.log"));
        let _ = std::fs::remove_dir(src_dir);
        let _ = std::fs::remove_dir(target_dir);
        let _ = std::fs::remove_dir(logs_dir);
        let _ = std::fs::remove_dir(app_dir);
    }

    #[test]
    fn test_longest_common_prefix_helper() {
        let v1 = vec!["process_payment".to_string(), "process_refund".to_string(), "process_webhook".to_string()];
        assert_eq!(longest_common_prefix(&v1), "process_");

        let v2 = vec!["apple".to_string(), "banana".to_string()];
        assert_eq!(longest_common_prefix(&v2), "");

        let v3 = vec!["test_fn".to_string()];
        assert_eq!(longest_common_prefix(&v3), "test_fn");
    }

    #[test]
    fn test_extract_functions_from_files_helper() {
        let temp_dir = std::env::temp_dir();
        let js_path = temp_dir.join("test_extract.js");
        let js_code = "function test_one() {} \n function test_two() {}";
        std::fs::write(&js_path, js_code).unwrap();

        let files = vec![js_path.to_string_lossy().to_string()];
        let funcs = extract_functions_from_files(&files);
        assert_eq!(funcs.len(), 2);
        assert_eq!(funcs[0].0, "test_one");
        assert_eq!(funcs[1].0, "test_two");

        let _ = std::fs::remove_file(js_path);
    }

    #[test]
    fn test_resolve_path_cases() {
        let temp_dir = std::env::temp_dir().canonicalize().unwrap_or_else(|_| std::env::temp_dir());
        let project_dir = temp_dir.join("test_code_analyzer_proj");
        std::fs::create_dir_all(&project_dir).unwrap();

        let nested_dir = project_dir.join("src");
        std::fs::create_dir_all(&nested_dir).unwrap();
        let file_path = nested_dir.join("json-engine.js");
        std::fs::write(&file_path, "function engine() {}").unwrap();

        let res1 = resolve_path("src/json-engine.js", &project_dir);
        assert!(res1.is_ok());
        assert_eq!(res1.unwrap().canonicalize().unwrap(), file_path.canonicalize().unwrap());

        let res2 = resolve_path("src/./json-engine.js", &project_dir);
        assert!(res2.is_ok());
        assert_eq!(res2.unwrap().canonicalize().unwrap(), file_path.canonicalize().unwrap());

        let res3 = resolve_path("src/../src/json-engine.js", &project_dir);
        assert!(res3.is_ok());
        assert_eq!(res3.unwrap().canonicalize().unwrap(), file_path.canonicalize().unwrap());

        let abs_path_str = file_path.to_string_lossy().to_string();
        let res4 = resolve_path(&abs_path_str, &project_dir);
        assert!(res4.is_ok());
        assert_eq!(res4.unwrap().canonicalize().unwrap(), file_path.canonicalize().unwrap());

        let res5 = resolve_path("json-engine.js", &nested_dir);
        assert!(res5.is_ok());
        assert_eq!(res5.unwrap().canonicalize().unwrap(), file_path.canonicalize().unwrap());

        let double_nested = "test_code_analyzer_proj/src/json-engine.js";
        let res6 = resolve_path(double_nested, &project_dir);
        assert!(res6.is_err());
        let err_msg = res6.unwrap_err();
        assert!(err_msg.contains("Could not read file"));
        assert!(err_msg.contains("Entered path: test_code_analyzer_proj/src/json-engine.js"));
        assert!(err_msg.contains("Current folder:"));
        assert!(err_msg.contains("Resolved path:"));
        assert!(err_msg.contains("Hint: Try src/json-engine.js"));

        let res7 = resolve_path("src/non-existent.js", &project_dir);
        assert!(res7.is_err());
        assert!(res7.unwrap_err().contains("Hint: Try a different relative path"));

        let _ = std::fs::remove_file(file_path);
        let _ = std::fs::remove_dir(nested_dir);
        let _ = std::fs::remove_dir(project_dir);
    }
}
