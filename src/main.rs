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

#[derive(Debug, Clone, PartialEq)]
struct FunctionResult {
    name: String,
    start_line: usize,
    end_line: usize,
    total_lines: usize,
    non_empty_lines: usize,
}

#[derive(Debug)]
struct AnalysisResult {
    source_file: String,
    language: String,
    analyzed_at: String,
    total_lines: usize,
    non_empty_lines: usize,
    import_lines: usize,
    function_definitions: usize,
    functions: Vec<FunctionResult>,
}

#[derive(Debug, PartialEq)]
enum Command {
    Analyze {
        source_file: String,
        log_file: Option<String>,
    },
    Compare {
        source_file: String,
        log_file: String,
    },
    Help,
}

fn parse_args(args: &[String]) -> Result<Command, String> {
    if args.len() < 2 {
        return Err("No command provided.".to_string());
    }
    match args[1].as_str() {
        "analyze" => {
            if args.len() == 3 {
                Ok(Command::Analyze {
                    source_file: args[2].clone(),
                    log_file: None,
                })
            } else if args.len() == 4 {
                Ok(Command::Analyze {
                    source_file: args[2].clone(),
                    log_file: Some(args[3].clone()),
                })
            } else {
                Err("Invalid number of arguments for 'analyze' command.".to_string())
            }
        }
        "compare" => {
            if args.len() == 4 {
                Ok(Command::Compare {
                    source_file: args[2].clone(),
                    log_file: args[3].clone(),
                })
            } else {
                Err("Invalid number of arguments for 'compare' command.".to_string())
            }
        }
        "--help" | "-h" | "help" => Ok(Command::Help),
        cmd => Err(format!("Unknown command '{}'.", cmd)),
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let program_name = args.get(0).map(|s| s.as_str()).unwrap_or("code-analyzer");

    match parse_args(&args) {
        Ok(Command::Analyze { source_file, log_file }) => {
            match analyze_file(&source_file) {
                Ok(result) => {
                    if let Some(log) = log_file {
                        if let Err(e) = save_analysis_result(&result, &log) {
                            eprintln!("Error: Could not save analysis result to '{}'. Details: {}", log, e);
                            process::exit(1);
                        }
                        println!("Analysis log saved to: {}", log);
                    } else {
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
                    }
                }
                Err(e) => {
                    eprintln!("Error: Could not read file '{}'. Details: {}", source_file, e);
                    process::exit(1);
                }
            }
        }
        Ok(Command::Compare { source_file, log_file }) => {
            if let Err(e) = compare_current_with_log(&source_file, &log_file) {
                eprintln!("Error during comparison. Details: {}", e);
                process::exit(1);
            }
        }
        Ok(Command::Help) => {
            print_usage(program_name);
            process::exit(0);
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
    eprintln!("  {} analyze <source-file> [log-file]", program_name);
    eprintln!("  {} compare <source-file> <baseline-log-file>", program_name);
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

/// Saves an `AnalysisResult` to a structured key-value log file.
fn save_analysis_result(result: &AnalysisResult, log_filename: &str) -> io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .append(true)
        .open(log_filename)?;
    writeln!(file, "source_file={}", result.source_file)?;
    writeln!(file, "language={}", result.language)?;
    writeln!(file, "analyzed_at={}", result.analyzed_at)?;
    writeln!(file, "total_lines={}", result.total_lines)?;
    writeln!(file, "non_empty_lines={}", result.non_empty_lines)?;
    writeln!(file, "import_lines={}", result.import_lines)?;
    writeln!(file, "function_definitions={}", result.function_definitions)?;
    for func in &result.functions {
        writeln!(
            file,
            "function={}:{}:{}:{}:{}",
            func.name, func.start_line, func.end_line, func.total_lines, func.non_empty_lines
        )?;
    }
    writeln!(file)?;
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
    let mut functions = Vec::new();

    // Since a log file can contain multiple reports appended, we need to extract the LAST report.
    // Let's first read all lines and group them into blocks separated by empty lines.
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
fn compare_current_with_log(source_file: &str, log_filename: &str) -> io::Result<()> {
    let old = parse_analysis_result(log_filename)?;
    let new = analyze_file(source_file)?;

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

    // Perform function-level comparisons
    println!("Function-Level Comparisons:");
    
    // Group functions by name
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

/// Helper to determine if a line in a Rust file represents a function definition.
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

/// Helper to determine if a line in a JavaScript file represents a function definition.
fn is_javascript_function_definition(line: &str) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_function_definitions() {
        // Valid definitions
        assert!(is_rust_function_definition("fn hello() {}"));
        assert!(is_rust_function_definition("   fn hello() {}")); // Indented
        assert!(is_rust_function_definition("fn_something_else") == false); // Not a word boundary
        assert!(is_rust_function_definition("fn()")); // fn followed by non-alphanumeric
        
        // Invalid definitions / calls
        assert!(!is_rust_function_definition("pub fn hello()")); // Starts with pub, not fn
        assert!(!is_rust_function_definition("let x = fn_name();"));
        assert!(!is_rust_function_definition("fn_call();"));
        assert!(!is_rust_function_definition("some other text"));
    }

    #[test]
    fn test_javascript_function_definitions() {
        // Traditional function
        assert!(is_javascript_function_definition("function add(a, b) {"));
        assert!(is_javascript_function_definition("   function add(a, b) {"));
        
        // Async function
        assert!(is_javascript_function_definition("async function fetchData(url) {"));
        assert!(is_javascript_function_definition("  async function fetchData(url) {"));
        
        // Basic arrow-function assignments (const, let, var)
        assert!(is_javascript_function_definition("const multiply = (a, b) => a * b;"));
        assert!(is_javascript_function_definition("let greet = name => `Hello, ${name}!`;"));
        assert!(is_javascript_function_definition("var power = (base, exp) => { return Math.pow(base, exp); };"));
        assert!(is_javascript_function_definition("   const asyncArrow = async () => {};"));
        
        // Exports
        assert!(is_javascript_function_definition("export function hello() {}"));
        assert!(is_javascript_function_definition("export default async function hello() {}"));
        assert!(is_javascript_function_definition("export const hello = () => {}"));

        // Do not count ordinary function calls or unrelated lines
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

        // Ensure clean state
        let _ = std::fs::remove_file(&log_path);

        let result1 = AnalysisResult {
            source_file: "test1.rs".to_string(),
            language: "Rust".to_string(),
            analyzed_at: "2026-08-05T00:15:00Z".to_string(),
            total_lines: 10,
            non_empty_lines: 8,
            import_lines: 2,
            function_definitions: 1,
            functions: vec![FunctionResult {
                name: "test_fn".to_string(),
                start_line: 4,
                end_line: 8,
                total_lines: 5,
                non_empty_lines: 4,
            }],
        };

        // 1. Verify file is created and contains correct fields
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

        // 2. Verify append behavior
        let result2 = AnalysisResult {
            source_file: "test2.js".to_string(),
            language: "JavaScript".to_string(),
            analyzed_at: "2026-08-05T00:16:00Z".to_string(),
            total_lines: 20,
            non_empty_lines: 15,
            import_lines: 3,
            function_definitions: 2,
            functions: vec![],
        };

        save_analysis_result(&result2, log_file_str).unwrap();

        let contents2 = std::fs::read_to_string(&log_path).unwrap();
        // Both entries should be present in the content
        assert!(contents2.contains("source_file=test1.rs"));
        assert!(contents2.contains("source_file=test2.js"));

        // parse_analysis_result should return the latest (second) result
        let parsed = parse_analysis_result(log_file_str).unwrap();
        assert_eq!(parsed.source_file, "test2.js");
        assert_eq!(parsed.language, "JavaScript");
        assert_eq!(parsed.analyzed_at, "2026-08-05T00:16:00Z");
        assert_eq!(parsed.total_lines, 20);
        assert_eq!(parsed.non_empty_lines, 15);
        assert_eq!(parsed.import_lines, 3);
        assert_eq!(parsed.function_definitions, 2);
        assert!(parsed.functions.is_empty());

        // Clean up
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
            function_definitions: 1,
            functions: vec![],
        };

        save_analysis_result(&old_res, old_str).unwrap();

        // Write a temporary source file for in-memory analysis
        std::fs::write(&source_path, "use std::io;\nfn hello() {}\nfn world() {}\n").unwrap();

        // Verify comparison works directly without creating a second log file
        let res = compare_current_with_log(source_str, old_str);
        assert!(res.is_ok());

        // Verify no log file was created for the source file
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

        // 1. Missing file
        let _ = std::fs::remove_file(&path);
        let err = parse_analysis_result(path_str).unwrap_err();
        assert!(err.to_string().contains("Could not open log file"));

        // Helper to write content
        let write_content = |content: &str| {
            std::fs::write(&path, content).unwrap();
        };

        // 2. Malformed line (no = symbol)
        write_content("source_file: test.rs\nlanguage=Rust\n");
        let err = parse_analysis_result(path_str).unwrap_err();
        assert!(err.to_string().contains("Malformed entry"));

        // 3. Missing required field (e.g. analyzed_at)
        write_content("source_file=test.rs\nlanguage=Rust\ntotal_lines=10\nnon_empty_lines=8\nimport_lines=2\nfunction_definitions=1\n");
        let err = parse_analysis_result(path_str).unwrap_err();
        assert!(err.to_string().contains("Missing required field 'analyzed_at'"));

        // 4. Invalid number format
        write_content("source_file=test.rs\nlanguage=Rust\nanalyzed_at=2026-08-05T00:15:00Z\ntotal_lines=abc\nnon_empty_lines=8\nimport_lines=2\nfunction_definitions=1\n");
        let err = parse_analysis_result(path_str).unwrap_err();
        assert!(err.to_string().contains("Invalid number for total_lines"));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_cli_parsing() {
        let program = "code-analyzer".to_string();

        // 1. Valid command: analyze <source-file>
        let args = vec![program.clone(), "analyze".to_string(), "sample.js".to_string()];
        assert_eq!(
            parse_args(&args),
            Ok(Command::Analyze {
                source_file: "sample.js".to_string(),
                log_file: None,
            })
        );

        // 2. Valid command: analyze <source-file> <log-file>
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
                log_file: Some("output.log".to_string()),
            })
        );

        // 3. Valid command: compare <source-file> <previous-log-file>
        let args = vec![
            program.clone(),
            "compare".to_string(),
            "sample.js".to_string(),
            "baseline.log".to_string(),
        ];
        assert_eq!(
            parse_args(&args),
            Ok(Command::Compare {
                source_file: "sample.js".to_string(),
                log_file: "baseline.log".to_string(),
            })
        );

        // 4. Missing command
        let args = vec![program.clone()];
        assert!(parse_args(&args).is_err());

        // 5. Invalid command name
        let args = vec![program.clone(), "invalid_cmd".to_string(), "sample.js".to_string()];
        assert!(parse_args(&args).is_err());

        // 6. Missing arguments for analyze
        let args = vec![program.clone(), "analyze".to_string()];
        assert!(parse_args(&args).is_err());

        // 7. Extra arguments for analyze
        let args = vec![
            program.clone(),
            "analyze".to_string(),
            "sample.js".to_string(),
            "output.log".to_string(),
            "extra".to_string(),
        ];
        assert!(parse_args(&args).is_err());

        // 8. Missing arguments for compare
        let args = vec![program.clone(), "compare".to_string(), "sample.js".to_string()];
        assert!(parse_args(&args).is_err());

        // 9. Extra arguments for compare
        let args = vec![
            program.clone(),
            "compare".to_string(),
            "sample.js".to_string(),
            "baseline.log".to_string(),
            "extra".to_string(),
        ];
        assert!(parse_args(&args).is_err());

        // 10. Help command variations
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

        // Should parse successfully without crashing, returning empty functions
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
}

