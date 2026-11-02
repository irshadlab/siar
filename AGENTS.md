# SIAR Agent Guidelines — Octocode Code Intelligence

When the user asks questions about anything in this codebase (architecture, crates, modules, functions, types, traits, bugs, dependencies, or workflows), always search and ground answers using the **Octocode MCP tools** (`ServerName: "octocode"` via `call_mcp_tool`).

## Research Loop
Follow the evidence-first loop: `SCOPE → ORIENT → SEARCH → READ EXACT → PROVE → ANSWER`

1. **Search (Fast Candidate Discovery)**
   - Use `localSearch` with `operation: "text"` to locate identifiers, error strings, structs, enums, functions, and comments across all crates.
   - Use `localSearch` with `operation: "structural"` to match AST patterns across Rust files.
   - Keep search paths scoped (e.g. `path: "/home/irshad/Projects/siar"` or target specific subdirectories like `crates/siar-identity-multidevice`).

2. **Orient & Inspect Structure**
   - Use `localSearch` with `operation: "tree"` or `operation: "files"` to view directory hierarchies and crate structures without massive directory dumps.
   - Use `localAnalyzeGraph` to map dependency graphs, module boundaries, and relationships between crates.

3. **Read Exact Implementation**
   - Use `localGetFileContent` with targeted line ranges or match windows to fetch the exact code bytes. Avoid loading entire multi-thousand-line files when answering specific questions.

4. **Prove via LSP**
   - Use `lspGetSemantics` (connected to `rust-analyzer`) for compiler-verified symbol definitions, call hierarchies (`callers`/`callees`), implementations, type definitions, and reference tracking.

5. **External & Upstream Code**
   - When the question involves upstream dependencies (e.g. OpenMLS, Slint, Tokio, Ring), use `ghSearch` and `ghGetFileContent` to ground answers in authoritative upstream code and documentation.

## Response Guidelines
- Ground all explanations in fetched code bytes, file paths, and exact line numbers.
- Provide clickable markdown file links (e.g., `[filename.rs](file:///home/irshad/Projects/siar/crates/.../filename.rs#L10-L30)`).

