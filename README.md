# Code Analyzer

A powerful Rust-based tool for analyzing and comparing source code files (supporting Rust `.rs` and JavaScript `.js`/`.mjs`/`.cjs`). It features a rich **Interactive Terminal UI (TUI)** and a robust **Command-Line Interface (CLI)**.

---

## Features

- **Interactive TUI:** A full-terminal dashboard to analyze files, select/compare multiple logs, manage (view/delete) saved logs, and change current workspaces.
- **Static Code Analysis:** Analyze line counts, import lines, and function definitions.
- **Function-Scoped Analysis:** Zoom into specific functions using the `--function` flag.
- **Multi-Log Comparison:** Compare 2 or more logs (or source files) to identify metrics diffs, trends, and anomalies.
- **Cross-Platform support:** Runs seamlessly on macOS and Linux.

---

## Installation

### macOS

#### Install from Source

You can build and install the binary directly from source:

```bash
cargo install --path .
```

*Note: Ensure that `~/.cargo/bin` is included in your system's `PATH` environment variable.*

#### Install Packaged Release Archive

Alternatively, download a packaged macOS release archive (`.tar.gz` or `.zip`) containing the precompiled `code-analyzer` binary, extract it, and copy the binary to a directory in your `PATH` (such as `/usr/local/bin`):

```bash
# Example for a .tar.gz archive
tar -xzf code-analyzer-macos.tar.gz
mv code-analyzer /usr/local/bin/
```

### Debian/Ubuntu Linux

Install the packaged `.deb` release using `apt`:

```bash
sudo apt install ./code-analyzer_<version>_<architecture>.deb
```

*Note: `.deb` packages are Linux-only and cannot be installed on macOS.*

### Verification

Verify that the installation was successful and that the binary is globally available:

```bash
code-analyzer --help
```

---

## Interactive TUI Mode

To launch the interactive TUI, run `code-analyzer` without any arguments:

```bash
code-analyzer
```

### Key Controls in TUI:
- **`[A]` Analyze source file(s):** Enter file paths (comma-separated for multiple files) to perform static analysis.
- **`[C]` Compare saved log files:** Opens a checklist to select multiple logs from the `logs/` directory and compare them.
- **`[L]` List or delete logs:** Browse, view contents of, or delete your saved run logs.
- **`[D]` Change current folder:** Change the active folder/workspace for analysis and log storage.
- **`[H]` Help:** View keyboard shortcuts and help commands.
- **`[Q] / Esc`:** Exit the current screen or quit the application.

---

## CLI Usage Examples

### 1. Analyze a Source File

Analyze a source file and display the results in the terminal:

```bash
code-analyzer analyze <source-file> [--output <path>] [--function <name>]
```

- **Basic Analysis:**
  ```bash
  code-analyzer analyze sample.js
  ```
- **Save Results:**
  ```bash
  code-analyzer analyze sample.js --output analysis.json
  ```
- **Function-Scoped Analysis:**
  ```bash
  code-analyzer analyze sample.js --function fetchData
  ```

### 2. Compare Source Files or Log Files

Compare two or more files (both raw source code files and parsed log files are supported):

```bash
code-analyzer compare <log1> <log2> [log3 ...] [--output <path>] [--function <name>]
```

- **Exactly 2 logs / files:** Compares the two sources, displaying details, anomalies, and metrics (including function diffs if comparing source files).
- **3 or more logs / files:** Automatically runs a multi-log comparison pipeline, rendering key findings summaries, comparison tables, and bar charts in the terminal.
- **Function-Scoped Comparison:**
  ```bash
  code-analyzer compare sample.js baseline.json --function multiply
  ```

---

## Build and Package

### macOS Release Binary

To compile the release binary:

```bash
cargo build --release
```

The compiled binary will be available at `./target/release/code-analyzer`.

### Debian Package

To build the Debian `.deb` package:

```bash
cargo deb
```

*Note: Building a Debian package using `cargo deb` must be performed in a Debian/Linux environment or a suitable CI/CD container.*
