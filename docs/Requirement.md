# Requirement Document for Code Analyzer (Rust)

I believe the less code you have, the easier it is to maintain the codebase. The sensible thing to do is to keep a track of how much lines of code you have at a file level.

When I give the following command, the tool should produce an output as follows.

code-analyzer sample.js

Output:

Lines of code: 10
Number of functions: 2
Number of imports: 1

Functions: 

- add: 2 lines
- helper: 2 lines

Running the code creates a .logs folder inside the folder where the file is present. Inside this folder, a report with the appropriate number should be created.

The command should output this in the terminal. [Not Done]

For now it only supports the .js files. I would like to add support for the following file types.

.ts - TypeScript
.tsx - TypeScript React
.jsx - React
.vue - Vue