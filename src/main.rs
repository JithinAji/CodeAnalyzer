use std::env;
use std::fs;
use std::path::Path;
use oxc_allocator::Allocator;
use oxc_parser::Parser;
use oxc_span::SourceType;
use oxc_ast_visit::Visit;
use oxc_syntax::scope::ScopeFlags;

struct LineIndex {
    line_starts: Vec<usize>,
}

impl LineIndex {
    fn new(source: &str) -> Self {
        let mut line_starts = vec![0];
        for (offset, c) in source.char_indices() {
            if c == '\n' {
                line_starts.push(offset + 1);
            }
        }
        Self { line_starts }
    }

    fn get_line(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(idx) => idx + 1,
            Err(idx) => idx,
        }
    }
}

struct FunctionInfo {
    name: String,
    lines: usize,
}

struct AnalyzerVisitor {
    line_index: LineIndex,
    imports_count: usize,
    functions: Vec<FunctionInfo>,
    current_var_name: Option<String>,
}

impl<'a> Visit<'a> for AnalyzerVisitor {
    fn visit_import_declaration(&mut self, decl: &oxc_ast::ast::ImportDeclaration<'a>) {
        self.imports_count += 1;
        oxc_ast_visit::walk::walk_import_declaration(self, decl);
    }

    fn visit_call_expression(&mut self, expr: &oxc_ast::ast::CallExpression<'a>) {
        if let oxc_ast::ast::Expression::Identifier(ident) = &expr.callee {
            if ident.name == "require" {
                self.imports_count += 1;
            }
        }
        oxc_ast_visit::walk::walk_call_expression(self, expr);
    }

    fn visit_function(&mut self, func: &oxc_ast::ast::Function<'a>, flags: ScopeFlags) {
        let name = if let Some(ref ident) = func.id {
            ident.name.to_string()
        } else {
            self.current_var_name.clone().unwrap_or_else(|| "<anonymous>".to_string())
        };
        let start_line = self.line_index.get_line(func.span.start as usize);
        let end_line = self.line_index.get_line(func.span.end as usize);
        let lines = if end_line >= start_line { end_line - start_line + 1 } else { 1 };
        self.functions.push(FunctionInfo { name, lines });

        let old_var = self.current_var_name.take();
        oxc_ast_visit::walk::walk_function(self, func, flags);
        self.current_var_name = old_var;
    }

    fn visit_arrow_function_expression(&mut self, expr: &oxc_ast::ast::ArrowFunctionExpression<'a>) {
        let name = self.current_var_name.clone().unwrap_or_else(|| "<anonymous>".to_string());
        let start_line = self.line_index.get_line(expr.span.start as usize);
        let end_line = self.line_index.get_line(expr.span.end as usize);
        let lines = if end_line >= start_line { end_line - start_line + 1 } else { 1 };
        self.functions.push(FunctionInfo { name, lines });

        let old_var = self.current_var_name.take();
        oxc_ast_visit::walk::walk_arrow_function_expression(self, expr);
        self.current_var_name = old_var;
    }

    fn visit_variable_declarator(&mut self, decl: &oxc_ast::ast::VariableDeclarator<'a>) {
        let old_var = self.current_var_name.clone();
        if let oxc_ast::ast::BindingPattern::BindingIdentifier(ref ident) = decl.id {
            self.current_var_name = Some(ident.name.to_string());
        }
        oxc_ast_visit::walk::walk_variable_declarator(self, decl);
        self.current_var_name = old_var;
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() != 2 {
        eprintln!("Usage: code-analyzer <filename>");
        std::process::exit(1);
    }

    let input_filename = &args[1];
    let path = Path::new(input_filename);

    if !path.exists() {
        eprintln!("Error: File '{}' does not exist.", input_filename);
        std::process::exit(1);
    }

    if path.extension().and_then(|s| s.to_str()) != Some("js") {
        eprintln!("Error: Only JavaScript (.js) files are accepted.");
        std::process::exit(1);
    }

    let source_content = fs::read_to_string(path)?;
    let total_lines = source_content.lines().count();

    let allocator = Allocator::default();
    let source_type = SourceType::mjs();
    let parser = Parser::new(&allocator, &source_content, source_type);
    let parser_ret = parser.parse();

    if !parser_ret.diagnostics.is_empty() {
        eprintln!("Error: Failed to parse JS file correctly.");
        for err in parser_ret.diagnostics {
            eprintln!("{:?}", err);
        }
        std::process::exit(1);
    }

    let line_index = LineIndex::new(&source_content);
    let mut visitor = AnalyzerVisitor {
        line_index,
        imports_count: 0,
        functions: Vec::new(),
        current_var_name: None,
    };

    visitor.visit_program(&parser_ret.program);

    // Create logs/ directory beside the input file
    let parent_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let logs_dir = parent_dir.join(".logs");
    fs::create_dir_all(&logs_dir)?;

    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("report");
    let mut n = 1;
    let report_path = loop {
        let candidate = logs_dir.join(format!("{}.{}.md", stem, n));
        if !candidate.exists() {
            break candidate;
        }
        n += 1;
    };

    // Generate markdown report
    let mut report = String::new();
    report.push_str(&format!("# Code Analysis Report for {}\n\n", path.file_name().unwrap().to_string_lossy()));
    report.push_str(&format!("- Total number of lines: {}\n", total_lines));
    report.push_str(&format!("- Total number of imported files: {}\n", visitor.imports_count));
    report.push_str(&format!("- Total number of functions: {}\n\n", visitor.functions.len()));
    report.push_str("## Functions\n");
    for func in &visitor.functions {
        report.push_str(&format!("- {}: {} lines\n", func.name, func.lines));
    }

    fs::write(&report_path, report)?;

    Ok(())
}
