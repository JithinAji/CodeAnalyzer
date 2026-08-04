# Code Analyzer CLI

A lightweight command-line tool in Rust for analyzing and comparing source files (supporting Rust `.rs` and JavaScript `.js`/`.mjs`/`.cjs`).

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

## Usage Examples

### Analyze a Source File

Analyze a source file and display the results in the terminal (including function-level metrics):

```bash
code-analyzer analyze <source-file>
```

Example:
```bash
code-analyzer analyze sample.js
```

Example Output:
```text
Total lines: 23
Non-empty lines: 17
Language: JavaScript
Import lines: 2
Function definitions: 4

Functions:
  - add (lines 6-8, total: 3, non-empty: 3)
  - fetchData (lines 11-14, total: 4, non-empty: 4)
  - multiply (lines 17-17, total: 1, non-empty: 1)
  - greet (lines 18-18, total: 1, non-empty: 1)
```

### Save Analysis Results to Log File

Run the analysis and save or append the results to a structured log file:

```bash
code-analyzer analyze <source-file> <log-file>
```

Example:
```bash
code-analyzer analyze sample.js analysis.log
```

### Compare Source File with Baseline Log

Analyze a source file in memory and compare it to a previously saved baseline log:

```bash
code-analyzer compare <source-file> <previous-log-file>
```

Example:
```bash
code-analyzer compare sample.js analysis.log
```

Example Output:
```text
Comparison: sample.js vs analysis.log

Source file:          sample.js (current) vs sample.js (baseline)
Language:             JavaScript (current) vs JavaScript (baseline)
Analyzed at:          2026-08-05T01:20:00Z (current) vs 2026-08-05T01:10:00Z (baseline)

Total lines:           23  → 23   (unchanged)
Non-empty lines:       17  → 17   (unchanged)
Import lines:          2   → 2    (unchanged)
Function definitions:  4   → 4    (unchanged)

Function-Level Comparisons:
  Changed/Unchanged Functions:
    - add: 3 → 3 lines (unchanged)
    - fetchData: 3 → 4 lines (+1, increased)
  Added Functions:
    - greet (lines 18-18, total: 1)
  Removed Functions:
    - oldHelper (total: 5)
```

---

## Build a Package

### macOS Release Binary

To compile the release binary on macOS:

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
