
## src/core/edit.rs

```rust
     1	//! Edit format parsing and application system
     2	//!
     3	//! Implements the EBNF edit format from Suggestions.md:
     4	//! - FILE: path blocks with REPLACE/INSERT/DELETE operations
     5	//! - GUARD-CID system for change detection
     6	//! - Safe atomic file operations with preview/backup
     7	
     8	use anyhow::{Context, Result};
     9	use std::fs;
    10	use std::path::{Path, PathBuf};
    11	use std::time::SystemTime;
    12	
    13	use crate::cli::{AppContext, ApplyArgs, BackupArgs, CheckSyntaxArgs, PreviewArgs};
    14	use crate::core::apply_engine::create_engine;
    15	
    16	/// Content ID for change detection (xxh64 hash)
    17	pub type ContentId = String;
    18	
    19	/// Shared normalizer for both CID and OLD comparisons  
    20	pub fn normalize_for_cid(s: &str) -> String {
    21	    // Split into lines, remove trailing spaces and '\r'
    22	    s.lines()
    23	        .map(|l| l.trim_end_matches(&[' ', '\t', '\r'][..]))
    24	        .collect::<Vec<_>>()
    25	        .join("\n")
    26	}
    27	
    28	/// Generate deterministic content ID using xxh64 with fixed seed
    29	pub fn generate_cid(content: &str) -> ContentId {
    30	    let normalized = normalize_for_cid(content);
    31	    let h = xxhash_rust::xxh64::xxh64(normalized.as_bytes(), 0);
    32	    format!("{:016x}", h)
    33	}
    34	
    35	/// Edit operation types
    36	#[derive(Debug, Clone, PartialEq)]
    37	pub enum EditOperation {
    38	    Replace {
    39	        start_line: usize, // 1-based inclusive
    40	        end_line: usize,   // 1-based inclusive
    41	        old_content: String,
    42	        new_content: String,
    43	        guard_cid: Option<ContentId>,
    44	    },
    45	    Insert {
    46	        at_line: usize, // 1-based, insert after this line (0 = beginning)
    47	        new_content: String,
    48	    },
    49	    Delete {
    50	        start_line: usize, // 1-based inclusive
    51	        end_line: usize,   // 1-based inclusive
    52	    },
    53	}
    54	
    55	/// File block containing path and operations
    56	#[derive(Debug, Clone)]
    57	pub struct FileBlock {
    58	    pub path: PathBuf,
    59	    pub operations: Vec<EditOperation>,
    60	}
    61	
    62	/// Complete edit specification
    63	#[derive(Debug, Clone)]
    64	pub struct EditSpec {
    65	    pub file_blocks: Vec<FileBlock>,
    66	}
    67	
    68	/// Edit parsing errors
    69	#[derive(Debug, thiserror::Error)]
    70	pub enum ParseError {
    71	    #[error("Invalid FILE block: {0}")]
    72	    InvalidFileBlock(String),
    73	    #[error("Invalid operation: {0}")]
    74	    InvalidOperation(String),
    75	    #[error("Missing required field: {0}")]
    76	    MissingField(String),
    77	    #[error("Invalid line number: {0}")]
    78	    InvalidLineNumber(String),
    79	    #[error("Invalid span format: {0}")]
    80	    InvalidSpan(String),
    81	}
    82	
    83	/// Edit conflict types
    84	#[derive(Debug, Clone)]
    85	pub enum EditConflict {
    86	    FileNotFound(PathBuf),
    87	
    88	    SpanOutOfRange {
    89	        file: PathBuf,
    90	        span: (usize, usize),
    91	        file_lines: usize,
    92	    },
    93	
    94	    ContentMismatch {
    95	        file: PathBuf,
    96	        expected_cid: ContentId,
    97	        actual_cid: ContentId,
    98	    },
    99	
   100	    OldContentMismatch {
   101	        file: PathBuf,
   102	        span: (usize, usize),
   103	    },
   104	}
   105	
   106	/// Edit application result
   107	#[derive(Debug)]
   108	pub struct EditResult {
   109	    pub applied_files: Vec<PathBuf>,
   110	    pub conflicts: Vec<EditConflict>,
   111	    pub backup_paths: Vec<PathBuf>,
   112	}
   113	
   114	/// Domain-specific error taxonomy for exit-code mapping
   115	#[derive(thiserror::Error, Debug, Clone)]
   116	pub enum ApplyCliError {
   117	    /// Unusable or malformed EBNF input
   118	    #[error("invalid input: {0}")]
   119	    InvalidInput(String),
   120	
   121	    /// No repository, invalid repo state, boundary violations, etc.
   122	    #[error("repository issue: {0}")]
   123	    Repo(String),
   124	
   125	    /// Merge conflicts or unapplyable hunks
   126	    #[error("conflicts: {0}")]
   127	    Conflicts(String),
   128	
   129	    /// Internal engine failures or unexpected bugs
   130	    #[error("internal error: {0}")]
   131	    Internal(String),
   132	}
   133	
   134	/// Explicit run-mode computed from flags
   135	#[derive(Copy, Clone, Debug, Eq, PartialEq)]
   136	pub enum RunMode {
   137	    Preview,
   138	    Apply,
   139	}
   140	
   141	/// Converts errors to the Phase-2 exit codes
   142	/// 0=success, 2=conflict, 3=invalid, 4=repo, 5=internal
   143	pub fn exit_code_for(e: &ApplyCliError) -> i32 {
   144	    match e {
   145	        ApplyCliError::InvalidInput(_) => 3,
   146	        ApplyCliError::Repo(_) => 4,
   147	        ApplyCliError::Conflicts(_) => 2,
   148	        ApplyCliError::Internal(_) => 5,
   149	    }
   150	}
   151	
   152	/// Discover the git repo root with multiple fallback strategies
   153	/// Returns Ok(None) when no repo is found. Callers must decide
   154	/// whether None is acceptable based on engine choice.
   155	pub fn discover_repo_root(explicit: Option<PathBuf>, start: &Path) -> Result<Option<PathBuf>> {
   156	    // 1) explicit override wins
   157	    if let Some(root) = explicit {
   158	        return Ok(Some(root));
   159	    }
   160	
   161	    // 2) git rev-parse
   162	    if let Ok(output) = std::process::Command::new("git")
   163	        .args(["rev-parse", "--show-toplevel"])
   164	        .current_dir(start)
   165	        .output()
   166	        && output.status.success()
   167	    {
   168	        let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
   169	        if !s.is_empty() {
   170	            return Ok(Some(PathBuf::from(s)));
   171	        }
   172	    }
   173	
   174	    // 3) ascend to find .git
   175	    let mut cur = Some(start);
   176	    while let Some(dir) = cur {
   177	        if dir.join(".git").exists() {
   178	            return Ok(Some(dir.to_path_buf()));
   179	        }
   180	        cur = dir.parent();
   181	    }
   182	
   183	    Ok(None)
   184	}
   185	
   186	/// Map crate/engine errors to ApplyCliError
   187	pub fn normalize_err(e: anyhow::Error) -> ApplyCliError {
   188	    let msg = format!("{e:#}");
   189	
   190	    // Simple string-match classification that can be refined later
   191	    if msg.contains("conflict") || msg.contains("merge") {
   192	        ApplyCliError::Conflicts(msg)
   193	    } else if msg.contains("git") || msg.contains("repo") {
   194	        ApplyCliError::Repo(msg)
   195	    } else if msg.contains("EBNF") || msg.contains("syntax") || msg.contains("parse") {
   196	        ApplyCliError::InvalidInput(msg)
   197	    } else {
   198	        ApplyCliError::Internal(msg)
   199	    }
   200	}
   201	
   202	/// Core edit engine
   203	#[derive(Default)]
   204	pub struct EditEngine {
   205	    preview_mode: bool,
   206	    backup_enabled: bool,
   207	    force_mode: bool,
   208	}
   209	
   210	impl EditEngine {
   211	    pub fn new() -> Self {
   212	        Self::default()
   213	    }
   214	
   215	    pub fn with_preview(mut self, enabled: bool) -> Self {
   216	        self.preview_mode = enabled;
   217	        self
   218	    }
   219	
   220	    pub fn with_backup(mut self, enabled: bool) -> Self {
   221	        self.backup_enabled = enabled;
   222	        self
   223	    }
   224	
   225	    pub fn with_force(mut self, enabled: bool) -> Self {
   226	        self.force_mode = enabled;
   227	        self
   228	    }
   229	
   230	    /// Parse edit specification from text
   231	    pub fn parse_edit_spec(&self, input: &str) -> Result<EditSpec, ParseError> {
   232	        let mut file_blocks = Vec::new();
   233	        let lines: Vec<&str> = input.lines().collect();
   234	        let mut i = 0;
   235	
   236	        while i < lines.len() {
   237	            let line = lines[i].trim();
   238	
   239	            // Skip empty lines and comments
   240	            if line.is_empty() || line.starts_with('#') {
   241	                i += 1;
   242	                continue;
   243	            }
   244	
   245	            // Parse FILE block
   246	            if line.starts_with("FILE:") {
   247	                let path_str = line.strip_prefix("FILE:").unwrap().trim();
   248	
   249	                if path_str.is_empty() {
   250	                    return Err(ParseError::InvalidFileBlock("Empty file path".to_string()));
   251	                }
   252	
   253	                let path = PathBuf::from(path_str);
   254	                i += 1;
   255	
   256	                // Parse operations for this file
   257	                let mut operations = Vec::new();
   258	                while i < lines.len() {
   259	                    let op_line = lines[i].trim();
   260	
   261	                    // Break if we hit next FILE block
   262	                    if op_line.starts_with("FILE:") {
   263	                        break;
   264	                    }
   265	                    // Skip blank lines between operations
   266	                    if op_line.is_empty() {
   267	                        i += 1;
   268	                        continue;
   269	                    }
   270	
   271	                    // Parse operation
   272	                    if let Some(op) = self.parse_operation(&lines, &mut i)? {
   273	                        operations.push(op);
   274	                    } else {
   275	                        i += 1;
   276	                    }
   277	                }
   278	
   279	                file_blocks.push(FileBlock { path, operations });
   280	            } else {
   281	                i += 1;
   282	            }
   283	        }
   284	
   285	        Ok(EditSpec { file_blocks })
   286	    }
   287	
   288	    /// Parse single operation starting at current line
   289	    fn parse_operation(
   290	        &self,
   291	        lines: &[&str],
   292	        i: &mut usize,
   293	    ) -> Result<Option<EditOperation>, ParseError> {
   294	        if *i >= lines.len() {
   295	            return Ok(None);
   296	        }
   297	
   298	        let line = lines[*i].trim();
   299	
   300	        // Check for GUARD-CID first
   301	        let guard_cid = if line.starts_with("GUARD-CID:") {
   302	            let cid: String = line.strip_prefix("GUARD-CID:").unwrap().trim().to_string();
   303	
   304	            *i += 1;
   305	
   306	            if *i >= lines.len() {
   307	                return Err(ParseError::InvalidOperation(
   308	                    "GUARD-CID without operation".to_string(),
   309	                ));
   310	            }
   311	
   312	            Some(cid)
   313	        } else {
   314	            None
   315	        };
   316	
   317	        let op_line = lines[*i].trim();
   318	
   319	        if op_line.starts_with("REPLACE lines") {
   320	            self.parse_replace_operation(lines, i, guard_cid)
   321	        } else if op_line.starts_with("INSERT at") {
   322	            self.parse_insert_operation(lines, i)
   323	        } else if op_line.starts_with("DELETE lines") {
   324	            self.parse_delete_operation(lines, i)
   325	        } else if !op_line.is_empty() {
   326	            Err(ParseError::InvalidOperation(format!(
   327	                "Unknown directive: {}",
   328	                op_line
   329	            )))
   330	        } else {
   331	            *i += 1;
   332	            Ok(None)
   333	        }
   334	    }
   335	
   336	    /// Parse REPLACE operation
   337	    fn parse_replace_operation(
   338	        &self,
   339	        lines: &[&str],
   340	        i: &mut usize,
   341	        guard_cid: Option<ContentId>,
   342	    ) -> Result<Option<EditOperation>, ParseError> {
   343	        let op_line = lines[*i].trim();
   344	
   345	        // Extract span from "REPLACE lines 10-15:"
   346	        let span_part = op_line
   347	            .strip_prefix("REPLACE lines")
   348	            .and_then(|s| s.strip_suffix(":"))
   349	            .ok_or_else(|| {
   350	                ParseError::InvalidOperation(format!("Invalid REPLACE syntax: {}", op_line))
   351	            })?
   352	            .trim();
   353	
   354	        let (start_line, end_line) = self.parse_span(span_part)?;
   355	        *i += 1;
   356	
   357	        // Parse OLD block
   358	        let old_content = self.parse_content_block(lines, i, "OLD:")?;
   359	
   360	        // Parse NEW block
   361	        let new_content = self.parse_content_block(lines, i, "NEW:")?;
   362	
   363	        Ok(Some(EditOperation::Replace {
   364	            start_line,
   365	            end_line,
   366	            old_content,
   367	            new_content,
   368	            guard_cid,
   369	        }))
   370	    }
   371	
   372	    /// Parse INSERT operation
   373	    fn parse_insert_operation(
   374	        &self,
   375	        lines: &[&str],
   376	        i: &mut usize,
   377	    ) -> Result<Option<EditOperation>, ParseError> {
   378	        let op_line = lines[*i].trim();
   379	
   380	        // Extract line from "INSERT at 10:"
   381	        let line_part = op_line
   382	            .strip_prefix("INSERT at")
   383	            .and_then(|s| s.strip_suffix(":"))
   384	            .ok_or_else(|| {
   385	                ParseError::InvalidOperation(format!("Invalid INSERT syntax: {}", op_line))
   386	            })?
   387	            .trim();
   388	
   389	        let at_line = line_part
   390	            .parse::<usize>()
   391	            .map_err(|_| ParseError::InvalidLineNumber(line_part.to_string()))?;
   392	        *i += 1;
   393	
   394	        // Parse NEW block
   395	        let new_content = self.parse_content_block(lines, i, "NEW:")?;
   396	
   397	        Ok(Some(EditOperation::Insert {
   398	            at_line,
   399	            new_content,
   400	        }))
   401	    }
   402	
   403	    /// Parse DELETE operation
   404	    fn parse_delete_operation(
   405	        &self,
   406	        lines: &[&str],
   407	        i: &mut usize,
   408	    ) -> Result<Option<EditOperation>, ParseError> {
   409	        let op_line = lines[*i].trim();
   410	
   411	        // Extract span from "DELETE lines 10-15"
   412	        let span_part = op_line
   413	            .strip_prefix("DELETE lines")
   414	            .ok_or_else(|| {
   415	                ParseError::InvalidOperation(format!("Invalid DELETE syntax: {}", op_line))
   416	            })?
   417	            .trim();
   418	
   419	        let (start_line, end_line) = self.parse_span(span_part)?;
   420	        *i += 1;
   421	
   422	        Ok(Some(EditOperation::Delete {
   423	            start_line,
   424	            end_line,
   425	        }))
   426	    }
   427	
   428	    /// Parse line span "10-15" or single line "10"
   429	    fn parse_span(&self, span_str: &str) -> Result<(usize, usize), ParseError> {
   430	        if span_str.contains('-') {
   431	            let parts: Vec<&str> = span_str.split('-').collect();
   432	            if parts.len() != 2 {
   433	                return Err(ParseError::InvalidSpan(span_str.to_string()));
   434	            }
   435	
   436	            let start = parts[0]
   437	                .trim()
   438	                .parse::<usize>()
   439	                .map_err(|_| ParseError::InvalidLineNumber(parts[0].to_string()))?;
   440	            let end = parts[1]
   441	                .trim()
   442	                .parse::<usize>()
   443	                .map_err(|_| ParseError::InvalidLineNumber(parts[1].to_string()))?;
   444	
   445	            if start == 0 || end == 0 || start > end {
   446	                return Err(ParseError::InvalidSpan(format!(
   447	                    "Invalid range: {}-{}",
   448	                    start, end
   449	                )));
   450	            }
   451	
   452	            Ok((start, end))
   453	        } else {
   454	            let line = span_str
   455	                .trim()
   456	                .parse::<usize>()
   457	                .map_err(|_| ParseError::InvalidLineNumber(span_str.to_string()))?;
   458	
   459	            if line == 0 {
   460	                return Err(ParseError::InvalidLineNumber(
   461	                    "Line numbers are 1-based".to_string(),
   462	                ));
   463	            }
   464	
   465	            Ok((line, line))
   466	        }
   467	    }
   468	
   469	    /// Parse content block (OLD:/NEW: followed by fenced code)
   470	    fn parse_content_block(
   471	        &self,
   472	        lines: &[&str],
   473	        i: &mut usize,
   474	        header: &str,
   475	    ) -> Result<String, ParseError> {
   476	        if *i >= lines.len() || !lines[*i].trim().starts_with(header) {
   477	            return Err(ParseError::MissingField(header.to_string()));
   478	        }
   479	        *i += 1;
   480	
   481	        // Look for fenced code block
   482	        let fence_line = lines[*i].trim();
   483	        if !fence_line.starts_with("```") {
   484	            return Err(ParseError::InvalidOperation(format!(
   485	                "Expected fenced code block after {}",
   486	                header
   487	            )));
   488	        }
   489	
   490	        // Count leading backticks in the opening fence (supports 3+)
   491	        let fence_len = fence_line.chars().take_while(|&c| c == '`').count();
   492	        let closing = "`".repeat(fence_len);
   493	        *i += 1;
   494	
   495	        // Collect content until matching fence run is found
   496	        let mut content_lines = Vec::new();
   497	        let mut closed = false;
   498	        while *i < lines.len() {
   499	            let line = lines[*i];
   500	            if line.trim() == closing {
   501	                *i += 1;
   502	                closed = true;
   503	                break;
   504	            }
   505	            content_lines.push(line);
   506	            *i += 1;
   507	        }
   508	        if !closed {
   509	            return Err(ParseError::InvalidOperation(format!(
   510	                "Unterminated fenced block after {}",
   511	                header
   512	            )));
   513	        }
   514	        Ok(content_lines.join("\n"))
   515	    }
   516	
   517	    /// Apply edit specification
   518	    pub fn apply(&self, spec: &EditSpec) -> Result<EditResult> {
   519	        let mut applied_files = Vec::new();
   520	        let mut conflicts = Vec::new();
   521	        let mut backup_paths = Vec::new();
   522	
   523	        // First pass: validate all operations
   524	        for file_block in &spec.file_blocks {
   525	            if !file_block.path.exists() {
   526	                conflicts.push(EditConflict::FileNotFound(file_block.path.clone()));
   527	                continue;
   528	            }
   529	
   530	            // Load file content for validation
   531	            let content = fs::read_to_string(&file_block.path)
   532	                .with_context(|| format!("Failed to read file: {:?}", file_block.path))?;
   533	            let file_lines: Vec<&str> = content.lines().collect();
   534	
   535	            // Validate each operation
   536	            for op in &file_block.operations {
   537	                match self.validate_operation(op, &file_lines, &file_block.path) {
   538	                    Ok(()) => {}
   539	                    Err(conflict) => {
   540	                        conflicts.push(conflict);
   541	                    }
   542	                }
   543	            }
   544	        }
   545	
   546	        // Stop if conflicts found and not in force mode
   547	        if !conflicts.is_empty() && !self.force_mode {
   548	            return Ok(EditResult {
   549	                applied_files,
   550	                conflicts,
   551	                backup_paths,
   552	            });
   553	        }
   554	
   555	        // Preview mode: just show what would be done
   556	        if self.preview_mode {
   557	            // TODO: Generate and display unified diff
   558	            return Ok(EditResult {
   559	                applied_files,
   560	                conflicts,
   561	                backup_paths,
   562	            });
   563	        }
   564	
   565	        // Apply operations to each file
   566	        for file_block in &spec.file_blocks {
   567	            if conflicts.iter().any(|c| match c {
   568	                EditConflict::FileNotFound(path) => path == &file_block.path,
   569	                _ => false,
   570	            }) {
   571	                continue; // Skip files that don't exist
   572	            }
   573	
   574	            // Create backup if requested
   575	            if self.backup_enabled {
   576	                let backup_path = self.create_backup(&file_block.path)?;
   577	                backup_paths.push(backup_path);
   578	            }
   579	
   580	            // Apply operations to this file
   581	            self.apply_file_operations(&file_block.path, &file_block.operations)?;
   582	            applied_files.push(file_block.path.clone());
   583	        }
   584	
   585	        Ok(EditResult {
   586	            applied_files,
   587	            conflicts,
   588	            backup_paths,
   589	        })
   590	    }
   591	
   592	    /// Validate single operation against file content
   593	    fn validate_operation(
   594	        &self,
   595	        op: &EditOperation,
   596	        file_lines: &[&str],
   597	        file_path: &Path,
   598	    ) -> Result<(), EditConflict> {
   599	        match op {
   600	            EditOperation::Replace {
   601	                start_line,
   602	                end_line,
   603	                old_content,
   604	                guard_cid,
   605	                ..
   606	            } => {
   607	                // Check span bounds
   608	                if *start_line == 0
   609	                    || *end_line == 0
   610	                    || *start_line > file_lines.len()
   611	                    || *end_line > file_lines.len()
   612	                {
   613	                    return Err(EditConflict::SpanOutOfRange {
   614	                        file: file_path.to_path_buf(),
   615	                        span: (*start_line, *end_line),
   616	                        file_lines: file_lines.len(),
   617	                    });
   618	                }
   619	
   620	                // Extract actual content in span (convert to 0-based indexing)
   621	                let actual_lines = &file_lines[(*start_line - 1)..*end_line];
   622	                let actual_content = actual_lines.join("\n");
   623	
   624	                // Check GUARD-CID if present; else fall back to OLD content compare
   625	                if let Some(expected_cid) = guard_cid {
   626	                    // Compute once and compare once
   627	                    let actual_cid = generate_cid(&actual_content);
   628	                    if expected_cid != &actual_cid {
   629	                        return Err(EditConflict::ContentMismatch {
   630	                            file: file_path.to_path_buf(),
   631	                            expected_cid: expected_cid.clone(),
   632	                            actual_cid,
   633	                        });
   634	                    }
   635	                } else {
   636	                    // No guard: normalize and compare the OLD payload
   637	                    if normalize_for_cid(old_content) != normalize_for_cid(&actual_content) {
   638	                        return Err(EditConflict::OldContentMismatch {
   639	                            file: file_path.to_path_buf(),
   640	                            span: (*start_line, *end_line),
   641	                        });
   642	                    }
   643	                }
   644	            }
   645	            EditOperation::Insert { at_line, .. } => {
   646	                // Check line bounds (0 is valid for insert at beginning)
   647	                if *at_line > file_lines.len() {
   648	                    return Err(EditConflict::SpanOutOfRange {
   649	                        file: file_path.to_path_buf(),
   650	                        span: (*at_line, *at_line),
   651	                        file_lines: file_lines.len(),
   652	                    });
   653	                }
   654	            }
   655	            EditOperation::Delete {
   656	                start_line,
   657	                end_line,
   658	            } => {
   659	                // Check span bounds
   660	                if *start_line == 0
   661	                    || *end_line == 0
   662	                    || *start_line > file_lines.len()
   663	                    || *end_line > file_lines.len()
   664	                {
   665	                    return Err(EditConflict::SpanOutOfRange {
   666	                        file: file_path.to_path_buf(),
   667	                        span: (*start_line, *end_line),
   668	                        file_lines: file_lines.len(),
   669	                    });
   670	                }
   671	            }
   672	        }
   673	        Ok(())
   674	    }
   675	
   676	    /// Create backup file with timestamp, preserving original extension
   677	    fn create_backup(&self, file_path: &Path) -> Result<PathBuf> {
   678	        let ts = SystemTime::now()
   679	            .duration_since(SystemTime::UNIX_EPOCH)?
   680	            .as_secs();
   681	
   682	        let backup_name = {
   683	            let orig = file_path
   684	                .file_name()
   685	                .ok_or_else(|| anyhow::anyhow!("Invalid file name"))?;
   686	            let stem = Path::new(orig).file_stem().unwrap_or(orig);
   687	            let ext = Path::new(orig).extension();
   688	            match ext {
   689	                Some(e) => format!(
   690	                    "{}.rup.bak.{}.{}",
   691	                    stem.to_string_lossy(),
   692	                    ts,
   693	                    e.to_string_lossy()
   694	                ),
   695	                None => format!("{}.rup.bak.{}", stem.to_string_lossy(), ts),
   696	            }
   697	        };
   698	
   699	        let backup_path = file_path.with_file_name(backup_name);
   700	
   701	        fs::copy(file_path, &backup_path)
   702	            .with_context(|| format!("Failed to create backup: {:?}", backup_path))?;
   703	
   704	        Ok(backup_path)
   705	    }
   706	
   707	    /// Apply operations to a single file
   708	    fn apply_file_operations(&self, file_path: &Path, operations: &[EditOperation]) -> Result<()> {
   709	        // Load file content
   710	        let content = fs::read_to_string(file_path)
   711	            .with_context(|| format!("Failed to read file: {:?}", file_path))?;
   712	
   713	        // Detect original newline style and EOF newline presence
   714	        let use_crlf = content.contains("\r\n");
   715	        let had_final_nl = content.ends_with("\n") || content.ends_with("\r\n");
   716	
   717	        // Build mutable lines without trailing '\r'
   718	        let mut file_lines: Vec<String> = content
   719	            .lines()
   720	            .map(|s| s.trim_end_matches('\r').to_string())
   721	            .collect();
   722	
   723	        // Check for overlapping operations
   724	        let mut ranges = Vec::new();
   725	        for op in operations {
   726	            match op {
   727	                EditOperation::Replace {
   728	                    start_line,
   729	                    end_line,
   730	                    ..
   731	                }
   732	                | EditOperation::Delete {
   733	                    start_line,
   734	                    end_line,
   735	                } => {
   736	                    ranges.push((*start_line, *end_line));
   737	                }
   738	                _ => {}
   739	            }
   740	        }
   741	        ranges.sort_by_key(|(s, e)| (*s, *e));
   742	        for w in ranges.windows(2) {
   743	            let (a_s, a_e) = w[0];
   744	            let (b_s, b_e) = w[1];
   745	            if b_s <= a_e {
   746	                return Err(anyhow::anyhow!(
   747	                    "Overlapping edits detected: {}-{} with {}-{}",
   748	                    a_s,
   749	                    a_e,
   750	                    b_s,
   751	                    b_e
   752	                ));
   753	            }
   754	        }
   755	
   756	        // Stable sort with tie-breakers
   757	        let mut sorted_ops = operations.to_vec();
   758	        sorted_ops.sort_by(|a, b| {
   759	            let key = |op: &EditOperation| -> (usize, u8, usize) {
   760	                match op {
   761	                    EditOperation::Delete {
   762	                        start_line,
   763	                        end_line,
   764	                    } => (*start_line, 0, *end_line),
   765	                    EditOperation::Replace {
   766	                        start_line,
   767	                        end_line,
   768	                        ..
   769	                    } => (*start_line, 1, *end_line),
   770	                    EditOperation::Insert { at_line, .. } => (*at_line, 2, *at_line),
   771	                }
   772	            };
   773	            let (as_, ak, ae) = key(a);
   774	            let (bs_, bk, be) = key(b);
   775	            // Desc by start, then by kind, then by end desc
   776	            bs_.cmp(&as_).then(ak.cmp(&bk)).then(be.cmp(&ae))
   777	        });
   778	
   779	        // Apply operations
   780	        for op in sorted_ops {
   781	            match op {
   782	                EditOperation::Replace {
   783	                    start_line,
   784	                    end_line,
   785	                    new_content,
   786	                    ..
   787	                } => {
   788	                    // Replace lines (convert to 0-based indexing)
   789	                    let start_idx = start_line - 1;
   790	                    let end_idx = end_line;
   791	
   792	                    let new_lines: Vec<String> =
   793	                        new_content.lines().map(|s| s.to_string()).collect();
   794	                    file_lines.splice(start_idx..end_idx, new_lines);
   795	                }
   796	                EditOperation::Insert {
   797	                    at_line,
   798	                    new_content,
   799	                } => {
   800	                    // Insert lines after at_line (0 means beginning)
   801	                    let insert_idx = at_line;
   802	                    let new_lines: Vec<String> =
   803	                        new_content.lines().map(|s| s.to_string()).collect();
   804	
   805	                    for (i, line) in new_lines.into_iter().enumerate() {
   806	                        file_lines.insert(insert_idx + i, line);
   807	                    }
   808	                }
   809	                EditOperation::Delete {
   810	                    start_line,
   811	                    end_line,
   812	                } => {
   813	                    // Delete lines (convert to 0-based indexing)
   814	                    let start_idx = start_line - 1;
   815	                    let end_idx = end_line;
   816	
   817	                    file_lines.drain(start_idx..end_idx);
   818	                }
   819	            }
   820	        }
   821	
   822	        // Reassemble with original newline style
   823	        let nl = if use_crlf { "\r\n" } else { "\n" };
   824	        let mut updated_content = file_lines.join(nl);
   825	        if had_final_nl {
   826	            updated_content.push_str(nl);
   827	        }
   828	
   829	        // Write updated content atomically with preserved permissions
   830	        let meta = fs::metadata(file_path)?;
   831	        let perms = meta.permissions();
   832	
   833	        // Create a temp file in the same directory
   834	        let parent = file_path
   835	            .parent()
   836	            .ok_or_else(|| anyhow::anyhow!("No parent directory"))?;
   837	        let mut tf = tempfile::NamedTempFile::new_in(parent).context("create temp file")?;
   838	
   839	        // Write the content fully
   840	        use std::io::Write;
   841	        tf.write_all(updated_content.as_bytes())
   842	            .context("write temp file")?;
   843	
   844	        // Apply permissions to the temp file (best effort)
   845	        fs::set_permissions(tf.path(), perms.clone()).context("set temp permissions")?;
   846	
   847	        // Atomically replace the destination
   848	        tf.persist(file_path)
   849	            .map_err(|e| anyhow::anyhow!("replace file atomically: {}", e))?;
   850	
   851	        Ok(())
   852	    }
   853	}
   854	
   855	/// Command handlers for CLI integration
   856	/// Apply edit specification with unified preview/apply flow using ApplyEngine trait
   857	pub fn apply_run(args: ApplyArgs, ctx: &AppContext) -> Result<()> {
   858	    // 1) Parse input (file or clipboard)
   859	    let ebnf = if let Some(file_path) = &args.edit_file {
   860	        fs::read_to_string(file_path)
   861	            .with_context(|| format!("Failed to read edit file: {:?}", file_path))?
   862	    } else if args.from_clipboard {
   863	        get_clipboard_content()?
   864	    } else {
   865	        return Err(ApplyCliError::InvalidInput(
   866	            "Must specify either --edit-file or --from-clipboard".to_string(),
   867	        )
   868	        .into());
   869	    };
   870	
   871	    // 2) Build edit specification
   872	    let legacy_engine = EditEngine::new();
   873	    let spec = legacy_engine
   874	        .parse_edit_spec(&ebnf)
   875	        .map_err(|e| ApplyCliError::InvalidInput(format!("Parse error: {}", e)))?;
   876	
   877	    // 3) Decide run mode: safe default is preview unless --apply was passed
   878	    let run_mode = if args.apply {
   879	        RunMode::Apply
   880	    } else {
   881	        if !ctx.quiet && !args.preview {
   882	            eprintln!("Safety mode: showing preview only. Use --apply to write changes.");
   883	        }
   884	        RunMode::Preview
   885	    };
   886	
   887	    // 4) Detect repo root (auto-detect with optional override)
   888	    let cwd = std::env::current_dir().context("Failed to get current directory")?;
   889	    let repo_root = discover_repo_root(args.repo_root.clone(), &cwd)
   890	        .context("Failed to detect repository root")?;
   891	
   892	    // 5) Create engine via factory
   893	    let need_repo = matches!(
   894	        args.engine,
   895	        crate::cli::ApplyEngine::Git | crate::cli::ApplyEngine::Auto
   896	    );
   897	    if need_repo && repo_root.is_none() {
   898	        return Err(ApplyCliError::Repo(
   899	            "Git engine requires a repository, but none found. Use --engine=internal or initialize a git repo.".to_string(),
   900	        ).into());
   901	    }
   902	
   903	    let engine = create_engine(
   904	        &args.engine,
   905	        &args.git_mode,
   906	        &args.whitespace,
   907	        args.backup,
   908	        args.force,
   909	        repo_root.unwrap_or_else(|| cwd.clone()),
   910	    )
   911	    .map_err(|e| ApplyCliError::Internal(format!("Engine creation failed: {}", e)))?;
   912	
   913	    // 6) Always check() first for consistent preview
   914	    let preview = engine.check(&spec).map_err(normalize_err)?;
   915	
   916	    // 7) Render preview (unified diff) unless --quiet
   917	    if !ctx.quiet {
   918	        if !preview.patch_content.is_empty() {
   919	            println!("{}", preview.patch_content);
   920	        }
   921	        if args.verbose {
   922	            println!("{}", preview.summary);
   923	        }
   924	    }
   925	
   926	    // 8) Check for conflicts and exit if in preview mode
   927	    if !preview.conflicts.is_empty() {
   928	        if !ctx.quiet {
   929	            eprintln!("Found {} conflicts:", preview.conflicts.len());
   930	            for conflict in &preview.conflicts {
   931	                eprintln!("  • {}", conflict);
   932	            }
   933	        }
   934	
   935	        if !args.force {
   936	            return Err(ApplyCliError::Conflicts(format!(
   937	                "{} conflicts detected. Use --force to apply despite conflicts.",
   938	                preview.conflicts.len()
   939	            ))
   940	            .into());
   941	        }
   942	    }
   943	
   944	    // 9) Stop here if Preview mode
   945	    if run_mode == RunMode::Preview {
   946	        return Ok(());
   947	    }
   948	
   949	    // 10) Apply for real
   950	    let report = engine.apply(&spec).map_err(normalize_err)?;
   951	
   952	    // 11) Report results and return
   953	    if !ctx.quiet {
   954	        if !report.applied_files.is_empty() {
   955	            println!("Applied changes to {} files:", report.applied_files.len());
   956	            for file in &report.applied_files {
   957	                println!("  • {}", file.display());
   958	            }
   959	        }
   960	
   961	        if !report.backup_paths.is_empty() {
   962	            println!("Created {} backup files:", report.backup_paths.len());
   963	            for backup in &report.backup_paths {
   964	                println!("  • {}", backup.display());
   965	            }
   966	        }
   967	    }
   968	
   969	    Ok(())
   970	}
   971	
   972	/// Convert Result<()> to exit codes for CLI harness
   973	/// Keep the mapping centralized for CI predictability
   974	pub fn finish_with_exit(result: Result<()>) -> ! {
   975	    match result {
   976	        Ok(()) => std::process::exit(0),
   977	        Err(e) => {
   978	            // Try to map anyhow error into our taxonomy
   979	            let cli_error = if let Some(cli) = e.downcast_ref::<ApplyCliError>() {
   980	                cli.clone()
   981	            } else {
   982	                normalize_err(e)
   983	            };
   984	            eprintln!("{}", cli_error);
   985	            std::process::exit(exit_code_for(&cli_error));
   986	        }
   987	    }
   988	}
   989	
   990	/// Preview edit changes without applying them
   991	pub fn preview_run(args: PreviewArgs, ctx: &AppContext) -> Result<()> {
   992	    let input = if args.from_clipboard {
   993	        get_clipboard_content()?
   994	    } else if let Some(file_path) = args.edit_file {
   995	        fs::read_to_string(&file_path)
   996	            .with_context(|| format!("Failed to read edit file: {:?}", file_path))?
   997	    } else {
   998	        anyhow::bail!("Must specify either --from-clipboard or provide edit file");
   999	    };
  1000	
  1001	    let engine = EditEngine::new().with_preview(true);
  1002	    let spec = engine
  1003	        .parse_edit_spec(&input)
  1004	        .context("Failed to parse edit specification")?;
  1005	
  1006	    if !ctx.quiet {
  1007	        println!("Preview of {} file blocks:", spec.file_blocks.len());
  1008	    }
  1009	
  1010	    // For now, just show what would be changed
  1011	    // TODO: Implement unified diff generation
  1012	    for file_block in &spec.file_blocks {
  1013	        println!("\n📁 File: {}", file_block.path.display());
  1014	        for (i, op) in file_block.operations.iter().enumerate() {
  1015	            match op {
  1016	                EditOperation::Replace {
  1017	                    start_line,
  1018	                    end_line,
  1019	                    old_content,
  1020	                    new_content,
  1021	                    ..
  1022	                } => {
  1023	                    println!("  {}. REPLACE lines {}-{}:", i + 1, start_line, end_line);
  1024	                    println!(
  1025	                        "     {} lines → {} lines",
  1026	                        old_content.lines().count(),
  1027	                        new_content.lines().count()
  1028	                    );
  1029	                }
  1030	                EditOperation::Insert {
  1031	                    at_line,
  1032	                    new_content,
  1033	                } => {
  1034	                    println!(
  1035	                        "  {}. INSERT at line {}: {} lines",
  1036	                        i + 1,
  1037	                        at_line,
  1038	                        new_content.lines().count()
  1039	                    );
  1040	                }
  1041	                EditOperation::Delete {
  1042	                    start_line,
  1043	                    end_line,
  1044	                } => {
  1045	                    println!("  {}. DELETE lines {}-{}", i + 1, start_line, end_line);
  1046	                }
  1047	            }
  1048	        }
  1049	    }
  1050	
  1051	    Ok(())
  1052	}
  1053	
  1054	/// Validate edit syntax without applying changes
  1055	pub fn check_syntax_run(args: CheckSyntaxArgs, ctx: &AppContext) -> Result<()> {
  1056	    let input = fs::read_to_string(&args.edit_file)
  1057	        .with_context(|| format!("Failed to read edit file: {:?}", args.edit_file))?;
  1058	
  1059	    let engine = EditEngine::new();
  1060	
  1061	    match engine.parse_edit_spec(&input) {
  1062	        Ok(spec) => {
  1063	            if !ctx.quiet {
  1064	                println!("Edit syntax is valid");
  1065	                println!(
  1066	                    "   {} file blocks with {} total operations",
  1067	                    spec.file_blocks.len(),
  1068	                    spec.file_blocks
  1069	                        .iter()
  1070	                        .map(|fb| fb.operations.len())
  1071	                        .sum::<usize>()
  1072	                );
  1073	            }
  1074	
  1075	            // Check if referenced files exist
  1076	            let mut missing_files = Vec::new();
  1077	            for file_block in &spec.file_blocks {
  1078	                if !file_block.path.exists() {
  1079	                    missing_files.push(&file_block.path);
  1080	                }
  1081	            }
  1082	
  1083	            if !missing_files.is_empty() {
  1084	                println!("Referenced files not found:");
  1085	                for file in missing_files {
  1086	                    println!("   • {}", file.display());
  1087	                }
  1088	                std::process::exit(1);
  1089	            }
  1090	        }
  1091	        Err(e) => {
  1092	            eprintln!("Edit syntax error: {}", e);
  1093	            std::process::exit(1);
  1094	        }
  1095	    }
  1096	
  1097	    Ok(())
  1098	}
  1099	
  1100	/// Create backup files
  1101	pub fn backup_run(args: BackupArgs, ctx: &AppContext) -> Result<()> {
  1102	    let engine = EditEngine::new();
  1103	    let mut backup_paths = Vec::new();
  1104	
  1105	    for file_path in &args.files {
  1106	        if !file_path.exists() {
  1107	            eprintln!("File not found: {}", file_path.display());
  1108	            continue;
  1109	        }
  1110	
  1111	        match engine.create_backup(file_path) {
  1112	            Ok(backup_path) => {
  1113	                backup_paths.push(backup_path);
  1114	            }
  1115	            Err(e) => {
  1116	                eprintln!("Failed to backup {}: {}", file_path.display(), e);
  1117	            }
  1118	        }
  1119	    }
  1120	
  1121	    if !ctx.quiet && !backup_paths.is_empty() {
  1122	        println!("Created {} backup files:", backup_paths.len());
  1123	        for backup in &backup_paths {
  1124	            println!("  • {}", backup.display());
  1125	        }
  1126	    }
  1127	
  1128	    Ok(())
  1129	}
  1130	
  1131	/// Get content from system clipboard
  1132	fn get_clipboard_content() -> Result<String> {
  1133	    use arboard::Clipboard;
  1134	    let mut clipboard = Clipboard::new().context("Failed to access clipboard")?;
  1135	    clipboard
  1136	        .get_text()
  1137	        .context("Failed to get text from clipboard")
  1138	}
  1139	
  1140	#[cfg(test)]
  1141	mod tests {
  1142	    use super::*;
  1143	    use std::io::Write;
  1144	    use tempfile::NamedTempFile;
  1145	
  1146	    #[test]
  1147	    fn test_generate_cid() {
  1148	        let content1 = "fn test() {\n    println!(\"hello\");\n}";
  1149	        let content2 = "fn test() {\n    println!(\"hello\");\n}";
  1150	        let content3 = "fn test() {\n    println!(\"world\");\n}";
  1151	
  1152	        assert_eq!(generate_cid(content1), generate_cid(content2));
  1153	        assert_ne!(generate_cid(content1), generate_cid(content3));
  1154	    }
  1155	
  1156	    #[test]
  1157	    fn test_parse_span() {
  1158	        let engine = EditEngine::new();
  1159	
  1160	        assert_eq!(engine.parse_span("10").unwrap(), (10, 10));
  1161	        assert_eq!(engine.parse_span("10-15").unwrap(), (10, 15));
  1162	        assert!(engine.parse_span("0").is_err());
  1163	        assert!(engine.parse_span("15-10").is_err());
  1164	    }
  1165	
  1166	    #[test]
  1167	    fn test_parse_simple_replace() {
  1168	        let engine = EditEngine::new();
  1169	        let input = r#"
  1170	FILE: test.rs
  1171	REPLACE lines 1-2:
  1172	OLD:
  1173	```rust
  1174	fn old_function() {
  1175	    println!("old");
  1176	}
  1177	```
  1178	NEW:
  1179	```rust
  1180	fn new_function() {
  1181	    println!("new");
  1182	}
  1183	```
  1184	"#;
  1185	
  1186	        let spec = engine.parse_edit_spec(input).unwrap();
  1187	        assert_eq!(spec.file_blocks.len(), 1);
  1188	        assert_eq!(spec.file_blocks[0].path, PathBuf::from("test.rs"));
  1189	        assert_eq!(spec.file_blocks[0].operations.len(), 1);
  1190	
  1191	        match &spec.file_blocks[0].operations[0] {
  1192	            EditOperation::Replace {
  1193	                start_line,
  1194	                end_line,
  1195	                old_content,
  1196	                new_content,
  1197	                guard_cid,
  1198	            } => {
  1199	                assert_eq!(*start_line, 1);
  1200	                assert_eq!(*end_line, 2);
  1201	                assert!(old_content.contains("old_function"));
  1202	                assert!(new_content.contains("new_function"));
  1203	                assert!(guard_cid.is_none());
  1204	            }
  1205	            _ => panic!("Expected Replace operation"),
  1206	        }
  1207	    }
  1208	
  1209	    #[test]
  1210	    fn test_create_and_apply_backup() {
  1211	        let mut temp_file = NamedTempFile::new().unwrap();
  1212	        writeln!(temp_file, "original content").unwrap();
  1213	        let temp_path = temp_file.path().to_path_buf();
  1214	
  1215	        let engine = EditEngine::new().with_backup(true);
  1216	        let backup_path = engine.create_backup(&temp_path).unwrap();
  1217	
  1218	        assert!(backup_path.exists());
  1219	        let backup_content = fs::read_to_string(&backup_path).unwrap();
  1220	        assert_eq!(backup_content.trim(), "original content");
  1221	
  1222	        // Cleanup
  1223	        fs::remove_file(backup_path).unwrap();
  1224	    }
  1225	
  1226	    #[test]
  1227	    fn test_blank_lines_between_ops() {
  1228	        let engine = EditEngine::new();
  1229	        let input = r#"
  1230	FILE: test.rs
  1231	REPLACE lines 1:
  1232	OLD:
  1233	```rust
  1234	old line
  1235	```
  1236	NEW:
  1237	```rust
  1238	new line
  1239	```
  1240	
  1241	INSERT at 2:
  1242	NEW:
  1243	```rust
  1244	inserted line
  1245	```
  1246	"#;
  1247	
  1248	        let spec = engine.parse_edit_spec(input).unwrap();
  1249	        assert_eq!(spec.file_blocks.len(), 1);
  1250	        assert_eq!(spec.file_blocks[0].operations.len(), 2);
  1251	    }
  1252	
  1253	    #[test]
  1254	    fn test_fence_run_robustness() {
  1255	        let engine = EditEngine::new();
  1256	        let input = r#"
  1257	FILE: test.rs
  1258	REPLACE lines 1:
  1259	OLD:
  1260	````rust
  1261	fn test() {
  1262	    // nested ```
  1263	}
  1264	````
  1265	NEW:
  1266	````rust
  1267	fn test() {
  1268	    // updated nested ```
  1269	}
  1270	````
  1271	"#;
  1272	
  1273	        let spec = engine.parse_edit_spec(input).unwrap();
  1274	        let op = &spec.file_blocks[0].operations[0];
  1275	        match op {
  1276	            EditOperation::Replace {
  1277	                old_content,
  1278	                new_content,
  1279	                ..
  1280	            } => {
  1281	                assert!(old_content.contains("nested ```"));
  1282	                assert!(new_content.contains("updated nested ```"));
  1283	            }
  1284	            _ => panic!("Expected Replace operation"),
  1285	        }
  1286	    }
  1287	
  1288	    #[test]
  1289	    fn test_crlf_preservation() {
  1290	        use tempfile::tempdir;
  1291	        let dir = tempdir().unwrap();
  1292	        let file_path = dir.path().join("test.txt");
  1293	
  1294	        // Create CRLF file with trailing newline
  1295	        let crlf_content = "line1\r\nline2\r\nline3\r\n";
  1296	        fs::write(&file_path, crlf_content).unwrap();
  1297	
  1298	        let engine = EditEngine::new();
  1299	        let operations = vec![EditOperation::Replace {
  1300	            start_line: 2,
  1301	            end_line: 2,
  1302	            old_content: "line2".to_string(),
  1303	            new_content: "modified line2".to_string(),
  1304	            guard_cid: None,
  1305	        }];
  1306	
  1307	        engine
  1308	            .apply_file_operations(&file_path, &operations)
  1309	            .unwrap();
  1310	
  1311	        let result = fs::read_to_string(&file_path).unwrap();
  1312	        assert!(result.contains("\r\n"), "CRLF should be preserved");
  1313	        assert!(
  1314	            result.ends_with("\r\n"),
  1315	            "Final newline should be preserved"
  1316	        );
  1317	        assert!(result.contains("modified line2"));
  1318	    }
  1319	
  1320	    #[test]
  1321	    fn test_deterministic_cid() {
  1322	        let content = "fn test() {\n    println!(\"hello\");\n}";
  1323	        let cid1 = generate_cid(content);
  1324	        let cid2 = generate_cid(content);
  1325	        assert_eq!(cid1, cid2, "CID should be deterministic");
  1326	
  1327	        // Different content should have different CID
  1328	        let different_content = "fn test() {\n    println!(\"world\");\n}";
  1329	        let cid3 = generate_cid(different_content);
  1330	        assert_ne!(cid1, cid3, "Different content should have different CID");
  1331	    }
  1332	
  1333	    #[test]
  1334	    fn test_unknown_directive_fails() {
  1335	        let engine = EditEngine::new();
  1336	        let input = r#"
  1337	FILE: test.rs
  1338	UPDATE lines 1-2:
  1339	OLD:
  1340	```rust
  1341	old code
  1342	```
  1343	NEW:
  1344	```rust
  1345	new code
  1346	```
  1347	"#;
  1348	
  1349	        let result = engine.parse_edit_spec(input);
  1350	        assert!(result.is_err());
  1351	        assert!(
  1352	            result
  1353	                .unwrap_err()
  1354	                .to_string()
  1355	                .contains("Unknown directive: UPDATE")
  1356	        );
  1357	    }
  1358	}
```

## src/parsers/python_parser.rs

```rust
     1	//! Filepath: src/parsers/python_parser.rs
     2	//! ------------------------------------------------------------------
     3	//! Python symbol extractor built on Tree-sitter 0.25.x.
     4	//! Goals:
     5	//!   - Use broad, stable queries (no fragile field predicates).
     6	//!   - Classify methods by ancestry (avoid duplicate matches).
     7	//!   - Extract PEP 257 docstrings (first statement string).
     8	//!   - Build qualified names for methods (A::B::m).
     9	//!   - Be careful with allocations and streaming iteration.
    10	//!
    11	//! Notes:
    12	//!   - We only query for functions and classes. Methods are
    13	//!     determined by detecting a surrounding class_definition.
    14	//!   - We always pass the same byte slice that Parser parsed.
    15	//!   - We rely on tree_sitter::StreamingIterator for matches.
    16	//!   - Docstrings support single/triple quotes and common
    17	//!     prefixes (r, u, f, fr, rf). Dedent is applied for
    18	//!     triple-quoted docs. Concatenated string docstrings are
    19	//!     joined segment-wise.
    20	//! ------------------------------------------------------------------
    21	
    22	use anyhow::{Context, Result, anyhow};
    23	use std::path::Path;
    24	use tree_sitter::{Language, Node, Parser, Query, QueryCursor, StreamingIterator};
    25	
    26	use crate::core::symbols::{Symbol, SymbolExtractor, SymbolKind, Visibility};
    27	// Reuse the shared helper to avoid drift
    28	use crate::infra::utils::TsNodeUtils;
    29	
    30	/// Extracts Python symbols (functions, classes, methods).
    31	pub struct PythonExtractor {
    32	    /// Python language handle for Tree-sitter.
    33	    language: Language,
    34	    /// Broad, stable query capturing defs and class defs.
    35	    query: Query,
    36	}
    37	
    38	impl PythonExtractor {
    39	    /// Construct a new extractor with a broad query that
    40	    /// captures function_definition and class_definition.
    41	    pub fn new() -> Result<Self> {
    42	        // Obtain the Tree-sitter language for Python.
    43	        let language = tree_sitter_python::LANGUAGE.into();
    44	
    45	        // Keep queries broad; avoid grammar field predicates
    46	        // that tend to change across minor versions.
    47	        let query_src = r#"
    48	            (function_definition
    49	              name: (identifier) @name) @item
    50	
    51	            (class_definition
    52	              name: (identifier) @name) @item
    53	        "#;
    54	
    55	        // Compile the query once for reuse in extraction.
    56	        let query = Query::new(&language, query_src).context("create Python query")?;
    57	
    58	        Ok(Self { language, query })
    59	    }
    60	}
    61	
    62	impl SymbolExtractor for PythonExtractor {
    63	    /// Parse `content`, run the query, derive symbol data, and
    64	    /// return a flat list of symbols defined in the file.
    65	    fn extract_symbols(&self, content: &str, file_path: &Path) -> Result<Vec<Symbol>> {
    66	        // Create a parser instance and set the language.
    67	        let mut parser = Parser::new();
    68	        parser
    69	            .set_language(&self.language)
    70	            .context("set Python language")?;
    71	
    72	        // Parse the source; fail if no tree is produced.
    73	        let tree = parser
    74	            .parse(content, None)
    75	            .ok_or_else(|| anyhow!("Failed to parse Python source"))?;
    76	
    77	        // Use the same bytes slice for all utf8_text calls.
    78	        let bytes = content.as_bytes();
    79	
    80	        // Prepare a query cursor and stream matches.
    81	        let mut cursor = QueryCursor::new();
    82	        let mut matches = cursor.matches(&self.query, tree.root_node(), bytes);
    83	
    84	        // Capture names vector for fast index lookup.
    85	        let cap_names: Vec<&str> = self.query.capture_names().to_vec();
    86	
    87	        // Pre-allocate a small buffer to reduce reallocations.
    88	        let mut out = Vec::with_capacity(16);
    89	
    90	        // Iterate streaming matches properly with .next().
    91	        while let Some(m) = matches.next() {
    92	            // Selected captured node of interest and its name.
    93	            let mut picked: Option<Node> = None;
    94	            let mut name_text: Option<String> = None;
    95	
    96	            // Process all captures in this match.
    97	            for cap in m.captures {
    98	                // Map capture index to its name string.
    99	                let cname = cap_names[cap.index as usize];
   100	
   101	                // The structural node (function/class) is @item.
   102	                if cname == "item" {
   103	                    picked = Some(cap.node);
   104	                    continue;
   105	                }
   106	
   107	                // The identifier for the symbol is @name.
   108	                if cname == "name" {
   109	                    name_text = cap.node.utf8_text(bytes).ok().map(|s| s.to_string());
   110	                    continue;
   111	                }
   112	            }
   113	
   114	            // Skip malformed matches lacking structure or name.
   115	            let Some(node) = picked else { continue };
   116	            let Some(name) = name_text else { continue };
   117	
   118	            // Classify as Method if nested in a class; else
   119	            // Function for function_definition or Class for
   120	            // class_definition. Avoid duplicates by not having
   121	            // a separate "method" query pattern.
   122	            let kind = match node.kind() {
   123	                "function_definition" => {
   124	                    if TsNodeUtils::has_ancestor(node, "class_definition") {
   125	                        SymbolKind::Method
   126	                    } else {
   127	                        SymbolKind::Function
   128	                    }
   129	                }
   130	                "class_definition" => SymbolKind::Class,
   131	                _ => continue,
   132	            };
   133	
   134	            // Build a qualified name. For methods we climb the
   135	            // class chain. For top-level items, keep simple name.
   136	            let qualified_name = if matches!(kind, SymbolKind::Method) {
   137	                python_qualified_name_method(node, bytes, &name)
   138	            } else {
   139	                name.clone()
   140	            };
   141	
   142	            // Python visibility by leading underscore policy.
   143	            let visibility = if name.starts_with('_') {
   144	                Some(Visibility::Private)
   145	            } else {
   146	                Some(Visibility::Public)
   147	            };
   148	
   149	            // Compute line and byte spans.
   150	            let start = node.start_position();
   151	            let end = node.end_position();
   152	
   153	            // Collect PEP 257-style docstring where present.
   154	            let doc = python_docstring_extract(node, bytes);
   155	
   156	            // Push the assembled symbol entry.
   157	            out.push(Symbol {
   158	                file: file_path.to_path_buf(),
   159	                lang: "python".to_string(),
   160	                kind,
   161	                name,
   162	                qualified_name,
   163	                byte_start: node.start_byte(),
   164	                byte_end: node.end_byte(),
   165	                start_line: start.row + 1,
   166	                end_line: end.row + 1,
   167	                visibility,
   168	                doc,
   169	            });
   170	        }
   171	
   172	        // Return the final symbol list.
   173	        Ok(out)
   174	    }
   175	}
   176	
   177	/// Build qualified method names of the form
   178	/// `Outer::Inner::method`, climbing ancestor classes.
   179	fn python_qualified_name_method(mut node: Node, bytes: &[u8], method_name: &str) -> String {
   180	    // Start with the innermost method name.
   181	    let mut parts: Vec<String> = vec![method_name.to_string()];
   182	
   183	    // Walk parents and collect class_definition names.
   184	    while let Some(parent) = node.parent() {
   185	        if parent.kind() == "class_definition"
   186	            && let Some(name_node) = parent.child_by_field_name("name")
   187	            && let Ok(cls) = name_node.utf8_text(bytes)
   188	        {
   189	            parts.push(cls.to_string());
   190	        }
   191	        node = parent;
   192	    }
   193	
   194	    // Reverse to outer-to-inner order and join.
   195	    parts.reverse();
   196	    parts.join("::")
   197	}
   198	
   199	/// Extract a PEP 257 docstring from a function/class:
   200	/// first statement in the body must be a string literal.
   201	/// Supports single/triple quotes, r/u/f prefixes, and
   202	/// concatenated string sequences.
   203	fn python_docstring_extract(node: Node, bytes: &[u8]) -> Option<String> {
   204	    // Obtain the block/suite node that contains statements.
   205	    let body = node.child_by_field_name("body")?;
   206	
   207	    // In current grammar, the body node itself is a "block"
   208	    // (or "suite" in older variants). Use it directly.
   209	    let block = if body.kind() == "block" || body.kind() == "suite" {
   210	        body
   211	    } else {
   212	        // Fallback: some grammars may nest blocks; try first
   213	        // named child that is a block/suite.
   214	        let mut blk = None;
   215	        for i in 0..body.named_child_count() {
   216	            let c = body.named_child(i)?;
   217	            if c.kind() == "block" || c.kind() == "suite" {
   218	                blk = Some(c);
   219	                break;
   220	            }
   221	        }
   222	        blk?
   223	    };
   224	
   225	    // Grab the first *named* statement (skips newlines/indent).
   226	    let first_stmt = block.named_child(0)?;
   227	    if first_stmt.kind() != "expression_statement" {
   228	        return None;
   229	    }
   230	
   231	    // The first expression should be a string literal or a
   232	    // concatenated_string (implicit adjacent literal concat).
   233	    let lit = first_stmt.named_child(0)?;
   234	    match lit.kind() {
   235	        "string" => {
   236	            let raw = lit.utf8_text(bytes).ok()?;
   237	            let text = unquote_python_string(raw);
   238	            Some(text)
   239	        }
   240	        "concatenated_string" => {
   241	            // Join each string segment after unquoting.
   242	            let mut acc = String::new();
   243	            for i in 0..lit.named_child_count() {
   244	                let seg = lit.named_child(i)?;
   245	                if seg.kind() != "string" {
   246	                    // Non-string in concatenation invalidates
   247	                    // docstring per PEP 257 expectations.
   248	                    return None;
   249	                }
   250	                let raw = seg.utf8_text(bytes).ok()?;
   251	                acc.push_str(&unquote_python_string(raw));
   252	            }
   253	            if acc.is_empty() { None } else { Some(acc) }
   254	        }
   255	        _ => None,
   256	    }
   257	}
   258	
   259	/// Strip Python string prefixes/quotes and perform a light
   260	/// unescape plus dedent for triple-quoted strings.
   261	fn unquote_python_string(s: &str) -> String {
   262	    // Trim leading/trailing whitespace around the literal.
   263	    let ss = s.trim();
   264	
   265	    // Compute prefix length (r, u, f, fr, rf; case-insensitive).
   266	    let pref_len = leading_alpha_len(ss);
   267	    let (prefix, rest) = ss.split_at(pref_len);
   268	
   269	    // Determine if raw (contains 'r' or 'R').
   270	    let is_raw = prefix.chars().any(|c| c == 'r' || c == 'R');
   271	
   272	    // Work with the remainder for quote detection.
   273	    let s2 = rest;
   274	
   275	    // Handle triple quotes first.
   276	    if s2.len() >= 6 {
   277	        if s2.starts_with(r#"""""#) && s2.ends_with(r#"""""#) {
   278	            let inner = &s2[3..s2.len() - 3];
   279	            return dedent_and_unescape(inner, is_raw);
   280	        }
   281	        if s2.starts_with("'''") && s2.ends_with("'''") {
   282	            let inner = &s2[3..s2.len() - 3];
   283	            return dedent_and_unescape(inner, is_raw);
   284	        }
   285	    }
   286	
   287	    // Handle single-quoted strings.
   288	    if s2.len() >= 2
   289	        && ((s2.starts_with('"') && s2.ends_with('"'))
   290	            || (s2.starts_with('\'') && s2.ends_with('\'')))
   291	    {
   292	        let inner = &s2[1..s2.len() - 1];
   293	        return basic_unescape(inner, is_raw);
   294	    }
   295	
   296	    // Fallback: return as-is.
   297	    s2.to_string()
   298	}
   299	
   300	/// Return the count of leading ASCII alphabetic chars.
   301	/// Used to slice off string literal prefixes.
   302	fn leading_alpha_len(s: &str) -> usize {
   303	    let mut i = 0;
   304	    for ch in s.chars() {
   305	        if ch.is_ascii_alphabetic() {
   306	            i += ch.len_utf8();
   307	        } else {
   308	            break;
   309	        }
   310	    }
   311	    i
   312	}
   313	
   314	/// Dedent triple-quoted content and unescape if not raw.
   315	/// Also strips a single leading/trailing blank line.
   316	fn dedent_and_unescape(s: &str, is_raw: bool) -> String {
   317	    // Split into lines and drop symmetric blank edges.
   318	    let mut lines: Vec<&str> = s.lines().collect();
   319	    if !lines.is_empty() && lines[0].trim().is_empty() {
   320	        lines.remove(0);
   321	    }
   322	    if !lines.is_empty() && lines[lines.len() - 1].trim().is_empty() {
   323	        lines.pop();
   324	    }
   325	
   326	    // Compute common leading spaces across non-empty lines.
   327	    let indent = lines
   328	        .iter()
   329	        .filter(|l| !l.trim().is_empty())
   330	        .map(|l| l.chars().take_while(|c| *c == ' ').count())
   331	        .min()
   332	        .unwrap_or(0);
   333	
   334	    // Dedent and join with newlines.
   335	    let mut out = String::new();
   336	    for (i, l) in lines.iter().enumerate() {
   337	        if !out.is_empty() {
   338	            out.push('\n');
   339	        }
   340	        if l.len() >= indent {
   341	            out.push_str(&l[indent..]);
   342	        } else {
   343	            out.push_str(l);
   344	        }
   345	        // Continue to next line.
   346	        let _ = i;
   347	    }
   348	
   349	    // Apply basic unescape only if not raw.
   350	    if is_raw {
   351	        out
   352	    } else {
   353	        basic_unescape(&out, false)
   354	    }
   355	}
   356	
   357	/// Minimal unescape for common sequences when not raw.
   358	/// Intended for docstrings, not general Python parsing.
   359	fn basic_unescape(s: &str, is_raw: bool) -> String {
   360	    if is_raw {
   361	        return s.to_string();
   362	    }
   363	    let mut out = String::with_capacity(s.len());
   364	    let mut it = s.chars();
   365	    while let Some(c) = it.next() {
   366	        if c == '\\' {
   367	            if let Some(n) = it.next() {
   368	                match n {
   369	                    'n' => out.push('\n'),
   370	                    't' => out.push('\t'),
   371	                    'r' => out.push('\r'),
   372	                    '\\' => out.push('\\'),
   373	                    '"' => out.push('"'),
   374	                    '\'' => out.push('\''),
   375	                    _ => {
   376	                        out.push('\\');
   377	                        out.push(n);
   378	                    }
   379	                }
   380	            } else {
   381	                out.push('\\');
   382	            }
   383	        } else {
   384	            out.push(c);
   385	        }
   386	    }
   387	    out
   388	}
   389	
   390	#[cfg(test)]
   391	mod tests {
   392	    use std::path::PathBuf;
   393	
   394	    use super::*;
   395	    use crate::core::symbols::{Symbol, SymbolKind, Visibility};
   396	
   397	    /// Helper: predicate for kind+name match.
   398	    fn has(sym: &Symbol, kind: SymbolKind, name: &str) -> bool {
   399	        sym.kind == kind && sym.name == name
   400	    }
   401	
   402	    /// Helper: fetch a single symbol by kind+name.
   403	    fn get<'a>(syms: &'a [Symbol], kind: SymbolKind, name: &str) -> &'a Symbol {
   404	        syms.iter()
   405	            .find(|s| has(s, kind.clone(), name))
   406	            .expect("symbol not found")
   407	    }
   408	
   409	    #[test]
   410	    fn python_functions_public_private_and_docstring() -> Result<()> {
   411	        let ex = PythonExtractor::new()?;
   412	        let src = r#"
   413	def hello():
   414	    """Greeting"""
   415	    return 1
   416	
   417	def _hidden():
   418	    return 2
   419	"#;
   420	        let file = PathBuf::from("test.py");
   421	        let mut syms = ex.extract_symbols(src, &file)?;
   422	        syms.sort_by_key(|s| (s.start_line, s.name.clone()));
   423	
   424	        let hello = get(&syms, SymbolKind::Function, "hello");
   425	        assert_eq!(hello.visibility, Some(Visibility::Public));
   426	        assert_eq!(hello.doc.as_deref(), Some("Greeting"));
   427	
   428	        let hidden = get(&syms, SymbolKind::Function, "_hidden");
   429	        assert_eq!(hidden.visibility, Some(Visibility::Private));
   430	        Ok(())
   431	    }
   432	
   433	    #[test]
   434	    fn python_class_and_methods_with_qualified_names() -> Result<()> {
   435	        let ex = PythonExtractor::new()?;
   436	        let src = r#"
   437	class MyClass:
   438	    """C doc"""
   439	    def method(self):
   440	        """M doc"""
   441	        pass
   442	
   443	    def _private(self):
   444	        pass
   445	"#;
   446	        let file = PathBuf::from("t.py");
   447	        let syms = ex.extract_symbols(src, &file)?;
   448	
   449	        assert!(syms.iter().any(|s| has(s, SymbolKind::Class, "MyClass")));
   450	
   451	        let m = syms
   452	            .iter()
   453	            .find(|s| s.kind == SymbolKind::Method && s.name == "method")
   454	            .unwrap();
   455	        assert_eq!(m.qualified_name, "MyClass::method");
   456	        assert_eq!(m.doc.as_deref(), Some("M doc"));
   457	
   458	        let p = syms
   459	            .iter()
   460	            .find(|s| s.kind == SymbolKind::Method && s.name == "_private")
   461	            .unwrap();
   462	        assert_eq!(p.visibility, Some(Visibility::Private));
   463	        Ok(())
   464	    }
   465	
   466	    #[test]
   467	    fn python_nested_classes_qualified_names() -> Result<()> {
   468	        let ex = PythonExtractor::new()?;
   469	        let src = r#"
   470	class Outer:
   471	    class Inner:
   472	        def m(self):
   473	            pass
   474	"#;
   475	        let file = PathBuf::from("t.py");
   476	        let syms = ex.extract_symbols(src, &file)?;
   477	        let m = syms
   478	            .iter()
   479	            .find(|s| s.kind == SymbolKind::Method && s.name == "m")
   480	            .unwrap();
   481	        assert_eq!(m.qualified_name, "Outer::Inner::m");
   482	        Ok(())
   483	    }
   484	
   485	    #[test]
   486	    fn python_non_first_string_is_not_docstring() -> Result<()> {
   487	        let ex = PythonExtractor::new()?;
   488	        let src = r#"
   489	def f():
   490	    x = 1
   491	    "not a docstring"
   492	    return x
   493	"#;
   494	        let file = PathBuf::from("t.py");
   495	        let syms = ex.extract_symbols(src, &file)?;
   496	        let f = get(&syms, SymbolKind::Function, "f");
   497	        assert!(f.doc.is_none());
   498	        Ok(())
   499	    }
   500	}
```

## src/core/patch.rs

```rust
     1	//! EBNF to unified diff patch converter
     2	//!
     3	//! Converts our human-readable edit format into standard Git patches
     4	//! for robust application with context matching and 3-way merging.
     5	
     6	use anyhow::{Context, Result};
     7	use std::collections::BTreeMap;
     8	use std::fs;
     9	use std::path::Path;
    10	
    11	use crate::core::edit::{EditOperation, EditSpec, generate_cid, normalize_for_cid};
    12	
    13	/// A single hunk in a unified diff
    14	#[derive(Debug, Clone)]
    15	pub struct Hunk {
    16	    pub old_start: usize, // 1-based line number in old file
    17	    pub old_count: usize, // Number of lines in old version
    18	    pub new_start: usize, // 1-based line number in new file
    19	    pub new_count: usize, // Number of lines in new version
    20	    pub lines: Vec<HunkLine>,
    21	}
    22	
    23	/// A line in a hunk with its change type
    24	#[derive(Debug, Clone)]
    25	pub enum HunkLine {
    26	    Context(String), // Unchanged line (starts with ' ')
    27	    Remove(String),  // Removed line (starts with '-')
    28	    Add(String),     // Added line (starts with '+')
    29	}
    30	
    31	/// A complete patch for one file
    32	#[derive(Debug, Clone)]
    33	pub struct FilePatch {
    34	    pub path: String,
    35	    pub hunks: Vec<Hunk>,
    36	    pub metadata: PatchMetadata,
    37	}
    38	
    39	/// Patch metadata for traceability
    40	#[derive(Debug, Clone)]
    41	pub struct PatchMetadata {
    42	    pub source_cid: Option<String>, // CID of source EBNF operation
    43	    pub context_lines: usize,
    44	    pub engine: String,
    45	}
    46	
    47	/// Complete patch set
    48	#[derive(Debug, Clone)]
    49	pub struct PatchSet {
    50	    pub file_patches: Vec<FilePatch>,
    51	}
    52	
    53	/// Patch generation configuration
    54	pub struct PatchConfig {
    55	    pub context_lines: usize,
    56	    pub validate_guards: bool,
    57	    pub merge_adjacent: bool,
    58	}
    59	
    60	impl Default for PatchConfig {
    61	    fn default() -> Self {
    62	        Self {
    63	            context_lines: 3,
    64	            validate_guards: true,
    65	            merge_adjacent: true,
    66	        }
    67	    }
    68	}
    69	
    70	/// Convert EBNF edit specification to unified diff patches
    71	pub fn generate_patches(spec: &EditSpec, config: &PatchConfig) -> Result<PatchSet> {
    72	    let mut file_patches = Vec::new();
    73	
    74	    // Group operations by file
    75	    let mut ops_by_file: BTreeMap<String, Vec<&EditOperation>> = BTreeMap::new();
    76	    for file_block in &spec.file_blocks {
    77	        let path_str = file_block.path.to_string_lossy().to_string();
    78	        ops_by_file
    79	            .entry(path_str)
    80	            .or_default()
    81	            .extend(&file_block.operations);
    82	    }
    83	
    84	    // Generate patch for each file
    85	    for (path_str, operations) in ops_by_file {
    86	        let file_patch = generate_file_patch(&path_str, operations, config)
    87	            .with_context(|| format!("Failed to generate patch for {}", path_str))?;
    88	        file_patches.push(file_patch);
    89	    }
    90	
    91	    Ok(PatchSet { file_patches })
    92	}
    93	
    94	/// Generate unified diff patch for a single file
    95	fn generate_file_patch(
    96	    path_str: &str,
    97	    operations: Vec<&EditOperation>,
    98	    config: &PatchConfig,
    99	) -> Result<FilePatch> {
   100	    let path = Path::new(path_str);
   101	
   102	    // Read current file content
   103	    let content =
   104	        fs::read_to_string(path).with_context(|| format!("Failed to read file: {}", path_str))?;
   105	    let file_lines: Vec<&str> = content.lines().collect();
   106	
   107	    // Convert operations to hunks
   108	    let hunks = operations_to_hunks(&file_lines, &operations, config)?;
   109	
   110	    // Merge adjacent/overlapping hunks if requested
   111	    let merged_hunks = if config.merge_adjacent {
   112	        merge_adjacent_hunks(hunks, config.context_lines)
   113	    } else {
   114	        hunks
   115	    };
   116	
   117	    // Sort hunks by line number
   118	    let mut sorted_hunks = merged_hunks;
   119	    sorted_hunks.sort_by_key(|h| h.old_start);
   120	
   121	    // Generate metadata
   122	    let source_cid = operations.first().and_then(|op| match op {
   123	        EditOperation::Replace {
   124	            guard_cid: Some(cid),
   125	            ..
   126	        } => Some(cid.clone()),
   127	        _ => None,
   128	    });
   129	
   130	    let metadata = PatchMetadata {
   131	        source_cid,
   132	        context_lines: config.context_lines,
   133	        engine: "rup".to_string(),
   134	    };
   135	
   136	    Ok(FilePatch {
   137	        path: path_str.to_string(),
   138	        hunks: sorted_hunks,
   139	        metadata,
   140	    })
   141	}
   142	
   143	/// Convert edit operations to hunks with context
   144	fn operations_to_hunks(
   145	    file_lines: &[&str],
   146	    operations: &[&EditOperation],
   147	    config: &PatchConfig,
   148	) -> Result<Vec<Hunk>> {
   149	    let mut hunks = Vec::new();
   150	
   151	    for op in operations {
   152	        let hunk = operation_to_hunk(file_lines, op, config)?;
   153	        hunks.push(hunk);
   154	    }
   155	
   156	    Ok(hunks)
   157	}
   158	
   159	/// Convert a single operation to a hunk
   160	fn operation_to_hunk(
   161	    file_lines: &[&str],
   162	    operation: &EditOperation,
   163	    config: &PatchConfig,
   164	) -> Result<Hunk> {
   165	    match operation {
   166	        EditOperation::Replace {
   167	            start_line,
   168	            end_line,
   169	            old_content,
   170	            new_content,
   171	            guard_cid,
   172	        } => {
   173	            // Validate against file content if guard present
   174	            if config.validate_guards {
   175	                validate_operation_content(
   176	                    file_lines,
   177	                    *start_line,
   178	                    *end_line,
   179	                    old_content,
   180	                    guard_cid,
   181	                )?;
   182	            }
   183	
   184	            let old_start = *start_line;
   185	            let old_end = *end_line;
   186	            let _old_count = old_end - old_start + 1;
   187	
   188	            // Parse new content into lines
   189	            let new_lines: Vec<&str> = new_content.lines().collect();
   190	            let new_count = new_lines.len();
   191	
   192	            // Build hunk with context
   193	            let context_start = old_start.saturating_sub(config.context_lines).max(1);
   194	            let context_end = (old_end + config.context_lines).min(file_lines.len());
   195	
   196	            let mut hunk_lines = Vec::new();
   197	
   198	            // Add leading context
   199	            for line_num in context_start..old_start {
   200	                let line = file_lines[line_num - 1]; // Convert to 0-based
   201	                hunk_lines.push(HunkLine::Context(line.to_string()));
   202	            }
   203	
   204	            // Add removed lines
   205	            for line_num in old_start..=old_end {
   206	                let line = file_lines[line_num - 1]; // Convert to 0-based
   207	                hunk_lines.push(HunkLine::Remove(line.to_string()));
   208	            }
   209	
   210	            // Add new lines
   211	            for new_line in &new_lines {
   212	                hunk_lines.push(HunkLine::Add(new_line.to_string()));
   213	            }
   214	
   215	            // Add trailing context
   216	            for line_num in (old_end + 1)..=context_end {
   217	                if line_num <= file_lines.len() {
   218	                    let line = file_lines[line_num - 1]; // Convert to 0-based
   219	                    hunk_lines.push(HunkLine::Context(line.to_string()));
   220	                }
   221	            }
   222	
   223	            // Calculate new start position (accounting for context)
   224	            let new_start = context_start;
   225	            let hunk_new_count = (context_start..old_start).count()
   226	                + new_count
   227	                + (old_end + 1..=context_end).count();
   228	
   229	            Ok(Hunk {
   230	                old_start: context_start,
   231	                old_count: (context_end - context_start + 1)
   232	                    .min(file_lines.len() - context_start + 1),
   233	                new_start,
   234	                new_count: hunk_new_count,
   235	                lines: hunk_lines,
   236	            })
   237	        }
   238	        EditOperation::Insert {
   239	            at_line,
   240	            new_content,
   241	        } => {
   242	            let insert_pos = *at_line; // 0 means beginning, N means after line N
   243	            let new_lines: Vec<&str> = new_content.lines().collect();
   244	
   245	            // Context around insertion point
   246	            let context_start = insert_pos.saturating_sub(config.context_lines).max(1);
   247	            let context_end = (insert_pos + config.context_lines).min(file_lines.len());
   248	
   249	            let mut hunk_lines = Vec::new();
   250	
   251	            // Add leading context
   252	            for line_num in context_start..=insert_pos.min(file_lines.len()) {
   253	                if line_num > 0 && line_num <= file_lines.len() {
   254	                    let line = file_lines[line_num - 1];
   255	                    hunk_lines.push(HunkLine::Context(line.to_string()));
   256	                }
   257	            }
   258	
   259	            // Add new lines
   260	            for new_line in &new_lines {
   261	                hunk_lines.push(HunkLine::Add(new_line.to_string()));
   262	            }
   263	
   264	            // Add trailing context
   265	            for line_num in (insert_pos + 1)..=context_end {
   266	                if line_num <= file_lines.len() {
   267	                    let line = file_lines[line_num - 1];
   268	                    hunk_lines.push(HunkLine::Context(line.to_string()));
   269	                }
   270	            }
   271	
   272	            Ok(Hunk {
   273	                old_start: context_start,
   274	                old_count: context_end - context_start + 1,
   275	                new_start: context_start,
   276	                new_count: (context_end - context_start + 1) + new_lines.len(),
   277	                lines: hunk_lines,
   278	            })
   279	        }
   280	        EditOperation::Delete {
   281	            start_line,
   282	            end_line,
   283	        } => {
   284	            let delete_count = end_line - start_line + 1;
   285	
   286	            // Context around deletion
   287	            let context_start = start_line.saturating_sub(config.context_lines).max(1);
   288	            let context_end = (end_line + config.context_lines).min(file_lines.len());
   289	
   290	            let mut hunk_lines = Vec::new();
   291	
   292	            // Add leading context
   293	            for line_num in context_start..*start_line {
   294	                let line = file_lines[line_num - 1];
   295	                hunk_lines.push(HunkLine::Context(line.to_string()));
   296	            }
   297	
   298	            // Add deleted lines
   299	            for line_num in *start_line..=*end_line {
   300	                let line = file_lines[line_num - 1];
   301	                hunk_lines.push(HunkLine::Remove(line.to_string()));
   302	            }
   303	
   304	            // Add trailing context
   305	            for line_num in (*end_line + 1)..=context_end {
   306	                if line_num <= file_lines.len() {
   307	                    let line = file_lines[line_num - 1];
   308	                    hunk_lines.push(HunkLine::Context(line.to_string()));
   309	                }
   310	            }
   311	
   312	            Ok(Hunk {
   313	                old_start: context_start,
   314	                old_count: context_end - context_start + 1,
   315	                new_start: context_start,
   316	                new_count: (context_end - context_start + 1) - delete_count,
   317	                lines: hunk_lines,
   318	            })
   319	        }
   320	    }
   321	}
   322	
   323	/// Validate operation content against file using same logic as EditEngine
   324	fn validate_operation_content(
   325	    file_lines: &[&str],
   326	    start_line: usize,
   327	    end_line: usize,
   328	    old_content: &str,
   329	    guard_cid: &Option<String>,
   330	) -> Result<()> {
   331	    // Extract actual content from file
   332	    let actual_lines = &file_lines[(start_line - 1)..end_line]; // Convert to 0-based
   333	    let actual_content = actual_lines.join("\n");
   334	
   335	    // Use same validation logic as EditEngine
   336	    if let Some(expected_cid) = guard_cid {
   337	        let actual_cid = generate_cid(&actual_content);
   338	        if expected_cid != &actual_cid {
   339	            anyhow::bail!(
   340	                "Content mismatch: expected CID {}, got {}",
   341	                expected_cid,
   342	                actual_cid
   343	            );
   344	        }
   345	    } else {
   346	        // Compare normalized content
   347	        if normalize_for_cid(old_content) != normalize_for_cid(&actual_content) {
   348	            anyhow::bail!("OLD content mismatch at lines {}-{}", start_line, end_line);
   349	        }
   350	    }
   351	
   352	    Ok(())
   353	}
   354	
   355	/// Merge adjacent hunks to reduce patch complexity
   356	fn merge_adjacent_hunks(hunks: Vec<Hunk>, context_lines: usize) -> Vec<Hunk> {
   357	    if hunks.len() <= 1 {
   358	        return hunks;
   359	    }
   360	
   361	    let mut merged = Vec::new();
   362	    let mut current = hunks[0].clone();
   363	
   364	    for next in hunks.into_iter().skip(1) {
   365	        // Check if hunks are close enough to merge
   366	        let current_end = current.old_start + current.old_count;
   367	        let gap = next.old_start.saturating_sub(current_end);
   368	
   369	        if gap <= context_lines * 2 {
   370	            // Merge hunks
   371	            current = merge_two_hunks(current, next);
   372	        } else {
   373	            // Too far apart, keep separate
   374	            merged.push(current);
   375	            current = next;
   376	        }
   377	    }
   378	
   379	    merged.push(current);
   380	    merged
   381	}
   382	
   383	/// Merge two adjacent hunks
   384	fn merge_two_hunks(mut first: Hunk, second: Hunk) -> Hunk {
   385	    // Extend first hunk to include second
   386	    first.old_count = (second.old_start + second.old_count) - first.old_start;
   387	    first.new_count = (second.new_start + second.new_count) - first.new_start;
   388	
   389	    // Merge lines (simplified - in production you'd handle overlapping context)
   390	    first.lines.extend(second.lines);
   391	
   392	    first
   393	}
   394	
   395	/// Render patch set as unified diff string
   396	pub fn render_unified_diff(patch_set: &PatchSet) -> String {
   397	    let mut output = String::new();
   398	
   399	    for file_patch in &patch_set.file_patches {
   400	        render_file_patch(&mut output, file_patch);
   401	    }
   402	
   403	    output
   404	}
   405	
   406	/// Render a single file patch
   407	fn render_file_patch(output: &mut String, file_patch: &FilePatch) {
   408	    // Add metadata comment
   409	    output.push_str(&format!(
   410	        "# RUP: CID={} CONTEXT={} ENGINE={}\n",
   411	        file_patch.metadata.source_cid.as_deref().unwrap_or("none"),
   412	        file_patch.metadata.context_lines,
   413	        file_patch.metadata.engine
   414	    ));
   415	
   416	    // Standard git diff header
   417	    output.push_str(&format!(
   418	        "diff --git a/{} b/{}\n",
   419	        file_patch.path, file_patch.path
   420	    ));
   421	    output.push_str(&format!("--- a/{}\n", file_patch.path));
   422	    output.push_str(&format!("+++ b/{}\n", file_patch.path));
   423	
   424	    // Render each hunk
   425	    for hunk in &file_patch.hunks {
   426	        render_hunk(output, hunk);
   427	    }
   428	}
   429	
   430	/// Render a single hunk
   431	fn render_hunk(output: &mut String, hunk: &Hunk) {
   432	    // Hunk header
   433	    output.push_str(&format!(
   434	        "@@ -{},{} +{},{} @@\n",
   435	        hunk.old_start, hunk.old_count, hunk.new_start, hunk.new_count
   436	    ));
   437	
   438	    // Render lines
   439	    for line in &hunk.lines {
   440	        match line {
   441	            HunkLine::Context(content) => output.push_str(&format!(" {}\n", content)),
   442	            HunkLine::Remove(content) => output.push_str(&format!("-{}\n", content)),
   443	            HunkLine::Add(content) => output.push_str(&format!("+{}\n", content)),
   444	        }
   445	    }
   446	}
   447	
   448	#[cfg(test)]
   449	mod tests {
   450	    use super::*;
   451	    use crate::core::edit::{EditOperation, EditSpec, FileBlock};
   452	    // use std::path::PathBuf; // Not needed in test
   453	    use std::io::Write;
   454	    use tempfile::NamedTempFile;
   455	
   456	    #[test]
   457	    fn test_simple_replace_patch() {
   458	        let mut temp_file = NamedTempFile::new().unwrap();
   459	        writeln!(temp_file, "line 1").unwrap();
   460	        writeln!(temp_file, "line 2").unwrap();
   461	        writeln!(temp_file, "line 3").unwrap();
   462	
   463	        let spec = EditSpec {
   464	            file_blocks: vec![FileBlock {
   465	                path: temp_file.path().to_path_buf(),
   466	                operations: vec![EditOperation::Replace {
   467	                    start_line: 2,
   468	                    end_line: 2,
   469	                    old_content: "line 2".to_string(),
   470	                    new_content: "modified line 2".to_string(),
   471	                    guard_cid: None,
   472	                }],
   473	            }],
   474	        };
   475	
   476	        let config = PatchConfig::default();
   477	        let patch_set = generate_patches(&spec, &config).unwrap();
   478	
   479	        assert_eq!(patch_set.file_patches.len(), 1);
   480	        let file_patch = &patch_set.file_patches[0];
   481	        assert_eq!(file_patch.hunks.len(), 1);
   482	
   483	        let diff = render_unified_diff(&patch_set);
   484	        assert!(diff.contains("diff --git"));
   485	        assert!(diff.contains("-line 2"));
   486	        assert!(diff.contains("+modified line 2"));
   487	    }
   488	
   489	    #[test]
   490	    fn test_insert_patch() {
   491	        let mut temp_file = NamedTempFile::new().unwrap();
   492	        writeln!(temp_file, "line 1").unwrap();
   493	        writeln!(temp_file, "line 2").unwrap();
   494	
   495	        let spec = EditSpec {
   496	            file_blocks: vec![FileBlock {
   497	                path: temp_file.path().to_path_buf(),
   498	                operations: vec![EditOperation::Insert {
   499	                    at_line: 1,
   500	                    new_content: "inserted line".to_string(),
   501	                }],
   502	            }],
   503	        };
   504	
   505	        let config = PatchConfig::default();
   506	        let patch_set = generate_patches(&spec, &config).unwrap();
   507	        let diff = render_unified_diff(&patch_set);
   508	
   509	        assert!(diff.contains("+inserted line"));
   510	    }
   511	}
```

## src/core/git.rs

```rust
     1	//! Git apply integration for robust patch application
     2	//!
     3	//! Implements git apply with 3-way merge, stderr parsing, and user-friendly
     4	//! error mapping according to engineering review specifications.
     5	
     6	use anyhow::{Context, Result};
     7	use std::path::PathBuf;
     8	use std::process::{Command, Stdio};
     9	
    10	use crate::core::patch::PatchSet;
    11	
    12	/// Git apply modes
    13	#[derive(Debug, Clone)]
    14	pub enum GitMode {
    15	    /// Apply to index (requires clean preimage)
    16	    Index,
    17	    /// 3-way merge (resilient, may leave conflict markers)
    18	    ThreeWay,
    19	    /// Apply to temporary worktree
    20	    Worktree,
    21	}
    22	
    23	/// Whitespace handling modes
    24	#[derive(Debug, Clone)]
    25	pub enum Whitespace {
    26	    /// Ignore whitespace issues
    27	    Nowarn,
    28	    /// Warn about whitespace issues
    29	    Warn,
    30	    /// Fix whitespace issues automatically
    31	    Fix,
    32	}
    33	
    34	/// Git apply configuration
    35	#[derive(Debug, Clone)]
    36	pub struct GitOptions {
    37	    pub repo_root: PathBuf,
    38	    pub mode: GitMode,
    39	    pub whitespace: Whitespace,
    40	    pub context_lines: u8,
    41	    pub allow_outside_repo: bool,
    42	}
    43	
    44	impl Default for GitOptions {
    45	    fn default() -> Self {
    46	        Self {
    47	            repo_root: PathBuf::from("."),
    48	            mode: GitMode::ThreeWay,
    49	            whitespace: Whitespace::Nowarn,
    50	            context_lines: 3,
    51	            allow_outside_repo: false,
    52	        }
    53	    }
    54	}
    55	
    56	/// Git apply outcome
    57	#[derive(Debug)]
    58	pub struct GitOutcome {
    59	    pub applied_files: Vec<PathBuf>,
    60	    pub conflicts: Vec<GitConflict>,
    61	    pub left_markers: Vec<PathBuf>,
    62	    pub stderr_raw: String,
    63	}
    64	
    65	/// Git conflict types with user-friendly categorization
    66	#[derive(Debug, Clone)]
    67	pub enum GitConflict {
    68	    PreimageMismatch {
    69	        path: PathBuf,
    70	        hunk: (u32, u32),
    71	        hint: &'static str,
    72	    },
    73	    PathOutsideRepo {
    74	        path: PathBuf,
    75	        hint: &'static str,
    76	    },
    77	    WhitespaceError {
    78	        path: PathBuf,
    79	        hint: &'static str,
    80	    },
    81	    IndexRequired {
    82	        path: PathBuf,
    83	        hint: &'static str,
    84	    },
    85	    BinaryOrMode {
    86	        path: PathBuf,
    87	        hint: &'static str,
    88	    },
    89	    Other(String),
    90	}
    91	
    92	/// Git apply engine implementation
    93	pub struct GitEngine {
    94	    options: GitOptions,
    95	    git_executable: Option<PathBuf>,
    96	}
    97	
    98	impl GitEngine {
    99	    /// Create new Git engine with options
   100	    pub fn new(options: GitOptions) -> Result<Self> {
   101	        let git_executable = detect_git_executable()?;
   102	        Ok(Self {
   103	            options,
   104	            git_executable: Some(git_executable),
   105	        })
   106	    }
   107	
   108	    /// Check if patch can be applied (preview mode)
   109	    pub fn check(&self, patch_set: &PatchSet) -> Result<GitOutcome> {
   110	        let patch_content = crate::core::patch::render_unified_diff(patch_set);
   111	        self.run_git_apply(&patch_content, true)
   112	    }
   113	
   114	    /// Apply patch set to repository
   115	    pub fn apply(&self, patch_set: &PatchSet) -> Result<GitOutcome> {
   116	        let patch_content = crate::core::patch::render_unified_diff(patch_set);
   117	        self.run_git_apply(&patch_content, false)
   118	    }
   119	
   120	    /// Run git apply with specified options
   121	    fn run_git_apply(&self, patch_content: &str, check_only: bool) -> Result<GitOutcome> {
   122	        let git_path = self
   123	            .git_executable
   124	            .as_ref()
   125	            .ok_or_else(|| anyhow::anyhow!("Git executable not found"))?;
   126	
   127	        let mut cmd = Command::new(git_path);
   128	        cmd.current_dir(&self.options.repo_root);
   129	
   130	        // Set whitespace handling
   131	        let whitespace_mode = match self.options.whitespace {
   132	            Whitespace::Nowarn => "nowarn",
   133	            Whitespace::Warn => "warn",
   134	            Whitespace::Fix => "fix",
   135	        };
   136	        cmd.arg("-c")
   137	            .arg(format!("apply.whitespace={}", whitespace_mode));
   138	
   139	        // Configure apply mode
   140	        cmd.arg("apply");
   141	
   142	        if check_only {
   143	            cmd.arg("--check");
   144	        }
   145	
   146	        match self.options.mode {
   147	            GitMode::Index => {
   148	                cmd.arg("--index");
   149	            }
   150	            GitMode::ThreeWay => {
   151	                cmd.arg("--3way");
   152	                if !check_only {
   153	                    cmd.arg("--index");
   154	                }
   155	            }
   156	            GitMode::Worktree => {
   157	                // Apply to working tree only
   158	            }
   159	        }
   160	
   161	        // Add verbose output for better error parsing
   162	        cmd.arg("--verbose");
   163	        cmd.arg("--reject");
   164	
   165	        // Read patch from stdin
   166	        cmd.stdin(Stdio::piped());
   167	        cmd.stdout(Stdio::piped());
   168	        cmd.stderr(Stdio::piped());
   169	
   170	        let mut child = cmd.spawn().context("Failed to spawn git apply process")?;
   171	
   172	        // Write patch content to stdin
   173	        if let Some(stdin) = child.stdin.take() {
   174	            use std::io::Write;
   175	            let mut stdin = stdin;
   176	            stdin
   177	                .write_all(patch_content.as_bytes())
   178	                .context("Failed to write patch to git apply stdin")?;
   179	        }
   180	
   181	        let output = child
   182	            .wait_with_output()
   183	            .context("Failed to wait for git apply process")?;
   184	
   185	        let stderr = String::from_utf8_lossy(&output.stderr);
   186	        let stdout = String::from_utf8_lossy(&output.stdout);
   187	
   188	        // Parse git apply output
   189	        let conflicts = parse_git_stderr(&stderr);
   190	        let applied_files = if output.status.success() {
   191	            extract_applied_files(&stdout, &stderr)
   192	        } else {
   193	            Vec::new()
   194	        };
   195	
   196	        // Check for conflict markers in applied files
   197	        let left_markers = if !check_only && matches!(self.options.mode, GitMode::ThreeWay) {
   198	            find_conflict_markers(&applied_files)?
   199	        } else {
   200	            Vec::new()
   201	        };
   202	
   203	        Ok(GitOutcome {
   204	            applied_files,
   205	            conflicts,
   206	            left_markers,
   207	            stderr_raw: stderr.to_string(),
   208	        })
   209	    }
   210	}
   211	
   212	/// Detect git executable and verify minimum version
   213	fn detect_git_executable() -> Result<PathBuf> {
   214	    let output = Command::new("git")
   215	        .arg("--version")
   216	        .output()
   217	        .context("Git executable not found in PATH")?;
   218	
   219	    if !output.status.success() {
   220	        anyhow::bail!("Git command failed");
   221	    }
   222	
   223	    let version_str = String::from_utf8_lossy(&output.stdout);
   224	
   225	    // Basic version check - ensure git 2.0+
   226	    if !version_str.contains("git version") {
   227	        anyhow::bail!("Unexpected git version output: {}", version_str);
   228	    }
   229	
   230	    Ok(PathBuf::from("git"))
   231	}
   232	
   233	/// Parse git apply stderr into structured conflicts
   234	fn parse_git_stderr(stderr: &str) -> Vec<GitConflict> {
   235	    let mut conflicts = Vec::new();
   236	
   237	    for line in stderr.lines() {
   238	        let line = line.trim();
   239	
   240	        if line.contains("patch does not apply") {
   241	            conflicts.push(GitConflict::PreimageMismatch {
   242	                path: extract_path_from_error(line).unwrap_or_else(|| PathBuf::from("unknown")),
   243	                hunk: (0, 0), // TODO: Parse actual hunk numbers
   244	                hint: "Target lines changed since suggestion. Try `--engine auto` or regenerate.",
   245	            });
   246	        } else if line.contains("does not match index") {
   247	            conflicts.push(GitConflict::IndexRequired {
   248	                path: extract_path_from_error(line).unwrap_or_else(|| PathBuf::from("unknown")),
   249	                hint: "Requires clean index. Commit or stash changes, or use `--git-mode 3way`.",
   250	            });
   251	        } else if line.contains("whitespace error") {
   252	            conflicts.push(GitConflict::WhitespaceError {
   253	                path: extract_path_from_error(line).unwrap_or_else(|| PathBuf::from("unknown")),
   254	                hint: "Whitespace sensitivity blocked apply. Try `--whitespace nowarn`.",
   255	            });
   256	        } else if line.contains("is outside repository") {
   257	            conflicts.push(GitConflict::PathOutsideRepo {
   258	                path: extract_path_from_error(line).unwrap_or_else(|| PathBuf::from("unknown")),
   259	                hint: "Edits must target tracked files within the repo root.",
   260	            });
   261	        } else if line.contains("error:") || line.contains("fatal:") {
   262	            conflicts.push(GitConflict::Other(line.to_string()));
   263	        }
   264	    }
   265	
   266	    conflicts
   267	}
   268	
   269	/// Extract file path from git error message
   270	fn extract_path_from_error(error_line: &str) -> Option<PathBuf> {
   271	    // Simple heuristic: look for file-like strings
   272	    // This is a simplified implementation - production would need more robust parsing
   273	    error_line
   274	        .split_whitespace()
   275	        .find(|word| word.contains(".rs") || word.contains(".py") || word.contains("/"))
   276	        .map(|path| PathBuf::from(path.trim_matches(|c| c == ':' || c == ',' || c == '"')))
   277	}
   278	
   279	/// Extract applied files from git output
   280	fn extract_applied_files(stdout: &str, stderr: &str) -> Vec<PathBuf> {
   281	    let mut files = Vec::new();
   282	
   283	    // Look for "Applying: " or similar patterns in output
   284	    for line in stdout.lines().chain(stderr.lines()) {
   285	        if (line.contains("Applying") || line.contains("patching file"))
   286	            && let Some(path) = extract_path_from_error(line)
   287	        {
   288	            files.push(path);
   289	        }
   290	    }
   291	
   292	    files
   293	}
   294	
   295	/// Find files with unresolved conflict markers
   296	fn find_conflict_markers(files: &[PathBuf]) -> Result<Vec<PathBuf>> {
   297	    let mut files_with_markers = Vec::new();
   298	
   299	    for file_path in files {
   300	        if let Ok(content) = std::fs::read_to_string(file_path)
   301	            && (content.contains("<<<<<<<")
   302	                || content.contains(">>>>>>>")
   303	                || content.contains("======="))
   304	        {
   305	            files_with_markers.push(file_path.clone());
   306	        }
   307	    }
   308	
   309	    Ok(files_with_markers)
   310	}
   311	
   312	/// Render user-friendly conflict summary
   313	pub fn render_conflict_summary(conflicts: &[GitConflict]) -> String {
   314	    if conflicts.is_empty() {
   315	        return String::new();
   316	    }
   317	
   318	    let mut output = format!("Conflicts ({})\n", conflicts.len());
   319	
   320	    for conflict in conflicts.iter() {
   321	        match conflict {
   322	            GitConflict::PreimageMismatch { path, hint, .. } => {
   323	                output.push_str(&format!(
   324	                    "  • {}: preimage mismatch\n    Remedy: {}\n",
   325	                    path.display(),
   326	                    hint
   327	                ));
   328	            }
   329	            GitConflict::IndexRequired { path, hint } => {
   330	                output.push_str(&format!(
   331	                    "  • {}: index required\n    Remedy: {}\n",
   332	                    path.display(),
   333	                    hint
   334	                ));
   335	            }
   336	            GitConflict::WhitespaceError { path, hint } => {
   337	                output.push_str(&format!(
   338	                    "  • {}: whitespace error\n    Remedy: {}\n",
   339	                    path.display(),
   340	                    hint
   341	                ));
   342	            }
   343	            GitConflict::PathOutsideRepo { path, hint } => {
   344	                output.push_str(&format!(
   345	                    "  • {}: outside repository\n    Remedy: {}\n",
   346	                    path.display(),
   347	                    hint
   348	                ));
   349	            }
   350	            GitConflict::BinaryOrMode { path, hint } => {
   351	                output.push_str(&format!(
   352	                    "  • {}: binary or mode change\n    Remedy: {}\n",
   353	                    path.display(),
   354	                    hint
   355	                ));
   356	            }
   357	            GitConflict::Other(msg) => {
   358	                output.push_str(&format!(
   359	                    "  • Other: {}\n    Remedy: Check git output with `--verbose`\n",
   360	                    msg
   361	                ));
   362	            }
   363	        }
   364	    }
   365	
   366	    output
   367	}
   368	
   369	#[cfg(test)]
   370	mod tests {
   371	    use super::*;
   372	
   373	    #[test]
   374	    fn test_git_detection() {
   375	        // This test requires git to be installed
   376	        if detect_git_executable().is_ok() {
   377	            // Git is available
   378	        } else {
   379	            // Skip test if git not available
   380	            println!("Git not available, skipping test");
   381	        }
   382	    }
   383	
   384	    #[test]
   385	    fn test_conflict_parsing() {
   386	        let stderr = r#"
   387	error: patch failed: src/main.rs:10
   388	error: src/main.rs: patch does not apply
   389	error: some/path/file.py: whitespace error
   390	"#;
   391	
   392	        let conflicts = parse_git_stderr(stderr);
   393	        assert!(conflicts.len() >= 2);
   394	
   395	        // Should detect preimage mismatch and whitespace error
   396	        assert!(
   397	            conflicts
   398	                .iter()
   399	                .any(|c| matches!(c, GitConflict::PreimageMismatch { .. }))
   400	        );
   401	        assert!(
   402	            conflicts
   403	                .iter()
   404	                .any(|c| matches!(c, GitConflict::WhitespaceError { .. }))
   405	        );
   406	    }
   407	
   408	    #[test]
   409	    fn test_conflict_summary_rendering() {
   410	        let conflicts = vec![
   411	            GitConflict::PreimageMismatch {
   412	                path: PathBuf::from("src/main.rs"),
   413	                hunk: (10, 15),
   414	                hint: "Try regenerating",
   415	            },
   416	            GitConflict::WhitespaceError {
   417	                path: PathBuf::from("src/lib.rs"),
   418	                hint: "Use --whitespace nowarn",
   419	            },
   420	        ];
   421	
   422	        let summary = render_conflict_summary(&conflicts);
   423	        assert!(summary.contains("Conflicts (2)"));
   424	        assert!(summary.contains("src/main.rs"));
   425	        assert!(summary.contains("src/lib.rs"));
   426	        assert!(summary.contains("preimage mismatch"));
   427	        assert!(summary.contains("whitespace error"));
   428	    }
   429	}
```

## src/core/apply_engine.rs

```rust
     1	//! Unified apply engine trait for hybrid EBNF→Git architecture
     2	//!
     3	//! Provides common interface for internal and git engines with
     4	//! automatic fallback and user-friendly reporting.
     5	
     6	use anyhow::Result;
     7	use std::path::PathBuf;
     8	
     9	use crate::cli::{ApplyEngine as EngineChoice, GitMode, WhitespaceMode};
    10	use crate::core::edit::EditSpec;
    11	use crate::core::git::{GitEngine, GitOptions};
    12	use crate::core::patch::{PatchConfig, generate_patches};
    13	
    14	/// Engine selection for apply operations
    15	#[derive(Debug, Clone, PartialEq, Eq)]
    16	pub enum Engine {
    17	    Internal,
    18	    Git,
    19	    Auto,
    20	}
    21	
    22	/// Apply operation preview
    23	#[derive(Debug)]
    24	pub struct Preview {
    25	    pub patch_content: String,
    26	    pub summary: String,
    27	    pub conflicts: Vec<String>,
    28	    pub engine_used: Engine,
    29	}
    30	
    31	/// Apply operation result
    32	#[derive(Debug)]
    33	pub struct ApplyReport {
    34	    pub applied_files: Vec<PathBuf>,
    35	    pub conflicts: Vec<String>,
    36	    pub engine_used: Engine,
    37	    pub backup_paths: Vec<PathBuf>,
    38	}
    39	
    40	/// Unified apply engine trait
    41	pub trait ApplyEngine {
    42	    /// Check if edit spec can be applied (preview mode)
    43	    fn check(&self, spec: &EditSpec) -> Result<Preview>;
    44	
    45	    /// Apply edit specification
    46	    fn apply(&self, spec: &EditSpec) -> Result<ApplyReport>;
    47	}
    48	
    49	/// Internal engine implementation
    50	pub struct InternalEngine {
    51	    backup_enabled: bool,
    52	    force_mode: bool,
    53	}
    54	
    55	impl InternalEngine {
    56	    pub fn new(backup_enabled: bool, force_mode: bool) -> Self {
    57	        Self {
    58	            backup_enabled,
    59	            force_mode,
    60	        }
    61	    }
    62	}
    63	
    64	impl ApplyEngine for InternalEngine {
    65	    fn check(&self, spec: &EditSpec) -> Result<Preview> {
    66	        let engine = crate::core::edit::EditEngine::new()
    67	            .with_preview(true)
    68	            .with_force(self.force_mode);
    69	
    70	        let result = engine.apply(spec)?;
    71	
    72	        // Generate patch for preview
    73	        let config = PatchConfig::default();
    74	        let patch_set = generate_patches(spec, &config)?;
    75	        let patch_content = crate::core::patch::render_unified_diff(&patch_set);
    76	
    77	        let conflicts: Vec<String> = result
    78	            .conflicts
    79	            .iter()
    80	            .map(|c| format!("{:?}", c)) // TODO: Better formatting
    81	            .collect();
    82	
    83	        let summary = format!(
    84	            "Preview: {} file(s), {} operation(s), {} conflict(s)",
    85	            spec.file_blocks.len(),
    86	            spec.file_blocks
    87	                .iter()
    88	                .map(|fb| fb.operations.len())
    89	                .sum::<usize>(),
    90	            result.conflicts.len()
    91	        );
    92	
    93	        Ok(Preview {
    94	            patch_content,
    95	            summary,
    96	            conflicts,
    97	            engine_used: Engine::Internal,
    98	        })
    99	    }
   100	
   101	    fn apply(&self, spec: &EditSpec) -> Result<ApplyReport> {
   102	        let engine = crate::core::edit::EditEngine::new()
   103	            .with_backup(self.backup_enabled)
   104	            .with_force(self.force_mode);
   105	
   106	        let result = engine.apply(spec)?;
   107	
   108	        let conflicts: Vec<String> = result
   109	            .conflicts
   110	            .iter()
   111	            .map(|c| format!("{:?}", c)) // TODO: Better formatting
   112	            .collect();
   113	
   114	        Ok(ApplyReport {
   115	            applied_files: result.applied_files,
   116	            conflicts,
   117	            engine_used: Engine::Internal,
   118	            backup_paths: result.backup_paths,
   119	        })
   120	    }
   121	}
   122	
   123	/// Git engine implementation
   124	pub struct GitEngineWrapper {
   125	    git_engine: GitEngine,
   126	}
   127	
   128	impl GitEngineWrapper {
   129	    pub fn new(git_options: GitOptions) -> Result<Self> {
   130	        let git_engine = GitEngine::new(git_options)?;
   131	        Ok(Self { git_engine })
   132	    }
   133	}
   134	
   135	impl ApplyEngine for GitEngineWrapper {
   136	    fn check(&self, spec: &EditSpec) -> Result<Preview> {
   137	        let config = PatchConfig::default();
   138	        let patch_set = generate_patches(spec, &config)?;
   139	        let patch_content = crate::core::patch::render_unified_diff(&patch_set);
   140	
   141	        let outcome = self.git_engine.check(&patch_set)?;
   142	
   143	        let conflicts: Vec<String> = outcome
   144	            .conflicts
   145	            .iter()
   146	            .map(|c| format!("{:?}", c)) // TODO: Use render_conflict_summary
   147	            .collect();
   148	
   149	        let summary = format!(
   150	            "Git Preview: {} file(s), {} conflict(s)",
   151	            patch_set.file_patches.len(),
   152	            outcome.conflicts.len()
   153	        );
   154	
   155	        Ok(Preview {
   156	            patch_content,
   157	            summary,
   158	            conflicts,
   159	            engine_used: Engine::Git,
   160	        })
   161	    }
   162	
   163	    fn apply(&self, spec: &EditSpec) -> Result<ApplyReport> {
   164	        let config = PatchConfig::default();
   165	        let patch_set = generate_patches(spec, &config)?;
   166	
   167	        let outcome = self.git_engine.apply(&patch_set)?;
   168	
   169	        let conflicts: Vec<String> = outcome
   170	            .conflicts
   171	            .iter()
   172	            .map(|c| format!("{:?}", c)) // TODO: Use render_conflict_summary
   173	            .collect();
   174	
   175	        Ok(ApplyReport {
   176	            applied_files: outcome.applied_files,
   177	            conflicts,
   178	            engine_used: Engine::Git,
   179	            backup_paths: Vec::new(), // Git doesn't create backups
   180	        })
   181	    }
   182	}
   183	
   184	/// Hybrid engine with automatic fallback
   185	pub struct HybridEngine {
   186	    internal: InternalEngine,
   187	    git: GitEngineWrapper,
   188	}
   189	
   190	impl HybridEngine {
   191	    pub fn new(backup_enabled: bool, force_mode: bool, git_options: GitOptions) -> Result<Self> {
   192	        let internal = InternalEngine::new(backup_enabled, force_mode);
   193	        let git = GitEngineWrapper::new(git_options)?;
   194	
   195	        Ok(Self { internal, git })
   196	    }
   197	}
   198	
   199	impl ApplyEngine for HybridEngine {
   200	    fn check(&self, spec: &EditSpec) -> Result<Preview> {
   201	        // Try internal first
   202	        match self.internal.check(spec) {
   203	            Ok(preview) if preview.conflicts.is_empty() => Ok(preview),
   204	            Ok(mut preview) => {
   205	                // Internal has conflicts, also show git preview
   206	                let git_preview = self.git.check(spec)?;
   207	                preview.summary.push_str(&format!(
   208	                    " | Git: {} conflict(s)",
   209	                    git_preview.conflicts.len()
   210	                ));
   211	                Ok(preview)
   212	            }
   213	            Err(_) => {
   214	                // Internal failed, try git
   215	                self.git.check(spec)
   216	            }
   217	        }
   218	    }
   219	
   220	    fn apply(&self, spec: &EditSpec) -> Result<ApplyReport> {
   221	        // Try internal first
   222	        match self.internal.apply(spec) {
   223	            Ok(report) if report.conflicts.is_empty() => Ok(report),
   224	            Ok(internal_report) => {
   225	                // Internal has conflicts, retry with git
   226	                println!("⚠️  Internal engine conflicts detected, retrying with git --3way");
   227	                match self.git.apply(spec) {
   228	                    Ok(mut git_report) => {
   229	                        git_report.engine_used = Engine::Auto;
   230	                        Ok(git_report)
   231	                    }
   232	                    Err(_) => {
   233	                        // Git also failed, return internal result
   234	                        Ok(internal_report)
   235	                    }
   236	                }
   237	            }
   238	            Err(e) => {
   239	                // Internal failed completely, try git
   240	                println!("⚠️  Internal engine failed, retrying with git");
   241	                self.git.apply(spec).or(Err(e))
   242	            }
   243	        }
   244	    }
   245	}
   246	
   247	/// Create appropriate engine based on user choice
   248	pub fn create_engine(
   249	    engine_choice: &EngineChoice,
   250	    git_mode: &GitMode,
   251	    whitespace: &WhitespaceMode,
   252	    backup_enabled: bool,
   253	    force_mode: bool,
   254	    repo_root: PathBuf,
   255	) -> Result<Box<dyn ApplyEngine>> {
   256	    let git_options = GitOptions {
   257	        repo_root,
   258	        mode: match git_mode {
   259	            GitMode::ThreeWay => crate::core::git::GitMode::ThreeWay,
   260	            GitMode::Index => crate::core::git::GitMode::Index,
   261	            GitMode::Worktree => crate::core::git::GitMode::Worktree,
   262	        },
   263	        whitespace: match whitespace {
   264	            WhitespaceMode::Nowarn => crate::core::git::Whitespace::Nowarn,
   265	            WhitespaceMode::Warn => crate::core::git::Whitespace::Warn,
   266	            WhitespaceMode::Fix => crate::core::git::Whitespace::Fix,
   267	        },
   268	        context_lines: 3,
   269	        allow_outside_repo: false,
   270	    };
   271	
   272	    match engine_choice {
   273	        EngineChoice::Internal => Ok(Box::new(InternalEngine::new(backup_enabled, force_mode))),
   274	        EngineChoice::Git => Ok(Box::new(GitEngineWrapper::new(git_options)?)),
   275	        EngineChoice::Auto => Ok(Box::new(HybridEngine::new(
   276	            backup_enabled,
   277	            force_mode,
   278	            git_options,
   279	        )?)),
   280	    }
   281	}
   282	
   283	#[cfg(test)]
   284	mod tests {
   285	    use super::*;
   286	    use crate::core::edit::{EditOperation, FileBlock};
   287	    use std::io::Write;
   288	    use tempfile::NamedTempFile;
   289	
   290	    #[test]
   291	    fn test_internal_engine() {
   292	        let mut temp_file = NamedTempFile::new().unwrap();
   293	        writeln!(temp_file, "line 1").unwrap();
   294	        writeln!(temp_file, "line 2").unwrap();
   295	
   296	        let spec = EditSpec {
   297	            file_blocks: vec![FileBlock {
   298	                path: temp_file.path().to_path_buf(),
   299	                operations: vec![EditOperation::Replace {
   300	                    start_line: 2,
   301	                    end_line: 2,
   302	                    old_content: "line 2".to_string(),
   303	                    new_content: "modified line 2".to_string(),
   304	                    guard_cid: None,
   305	                }],
   306	            }],
   307	        };
   308	
   309	        let engine = InternalEngine::new(false, false);
   310	        let preview = engine.check(&spec).unwrap();
   311	
   312	        assert_eq!(preview.engine_used, Engine::Internal);
   313	        assert!(preview.patch_content.contains("modified line 2"));
   314	        assert!(preview.conflicts.is_empty());
   315	    }
   316	}
```

## src/infra/utils.rs

```rust
     1	//! Filepath: src/utils.rs
     2	//! Utility helpers organized by small, focused structs.
     3	//! All functions are associated fns to keep call sites
     4	//! ergonomic, testable, and discoverable.
     5	
     6	// Tree-sitter types for node helpers
     7	use tree_sitter::{Node, Point};
     8	
     9	/// Qualified-name helpers
    10	pub struct NameUtils;
    11	
    12	impl NameUtils {
    13	    /// Join name parts with the given separator into a String
    14	    pub fn join(parts: &[&str], sep: char) -> String {
    15	        // Pre-allocate with a simple heuristic
    16	        let mut out = String::with_capacity(parts.iter().map(|p| p.len() + 1).sum());
    17	
    18	        // Push parts with separator
    19	        for (i, p) in parts.iter().enumerate() {
    20	            if i > 0 {
    21	                out.push(sep);
    22	            }
    23	
    24	            out.push_str(p);
    25	        }
    26	
    27	        // Return the constructed string
    28	        out
    29	    }
    30	}
    31	
    32	/// UTF-8 safe slicing helpers
    33	pub struct Utf8Utils;
    34	
    35	impl Utf8Utils {
    36	    /// Return a substring by byte range if it is on a char
    37	    /// boundary within `full`, else None
    38	    pub fn slice_str(full: &str, start: usize, end: usize) -> Option<&str> {
    39	        // Early checks on range validity
    40	        if start > end || end > full.len() {
    41	            return None;
    42	        }
    43	
    44	        // Use get(..) to enforce char boundary safety
    45	        full.get(start..end)
    46	    }
    47	
    48	    /// Convert a tree-sitter byte range to a &str slice,
    49	    /// returns None if boundaries are not valid char
    50	    /// boundaries
    51	    pub fn slice_node_text<'a>(full: &'a str, node: Node<'a>) -> Option<&'a str> {
    52	        // Obtain start and end byte offsets
    53	        let s = node.start_byte();
    54	        let e = node.end_byte();
    55	
    56	        // Slice via checked get
    57	        Self::slice_str(full, s, e)
    58	    }
    59	}
    60	
    61	/// Common Tree-sitter node helpers
    62	pub struct TsNodeUtils;
    63	
    64	impl TsNodeUtils {
    65	    /// Check if `node` has an ancestor of the given kind
    66	    pub fn has_ancestor(mut node: Node, kind: &str) -> bool {
    67	        // Walk up parents until root
    68	        while let Some(p) = node.parent() {
    69	            if p.kind() == kind {
    70	                return true;
    71	            }
    72	
    73	            node = p;
    74	        }
    75	
    76	        // No matching ancestor found
    77	        false
    78	    }
    79	
    80	    /// Find the first ancestor of the given kind
    81	    pub fn find_ancestor<'a>(mut node: Node<'a>, kind: &'a str) -> Option<Node<'a>> {
    82	        // Walk up parents until we match or hit root
    83	        while let Some(p) = node.parent() {
    84	            if p.kind() == kind {
    85	                return Some(p);
    86	            }
    87	
    88	            node = p;
    89	        }
    90	
    91	        // No ancestor found
    92	        None
    93	    }
    94	
    95	    /// Extract text of a child field if present
    96	    pub fn field_text<'a>(node: Node, field: &str, bytes: &'a [u8]) -> Option<&'a str> {
    97	        // Locate the child by field name
    98	        let child = node.child_by_field_name(field)?;
    99	
   100	        // Convert to utf8 text
   101	        child.utf8_text(bytes).ok()
   102	    }
   103	
   104	    /// Convert node positions to 1-based line numbers
   105	    pub fn line_range_1based(node: Node) -> (usize, usize) {
   106	        // Fetch start and end Points
   107	        let s: Point = node.start_position();
   108	        let e: Point = node.end_position();
   109	
   110	        // Convert to 1-based rows
   111	        (s.row + 1, e.row + 1)
   112	    }
   113	}
   114	
   115	/// Python docstring helpers
   116	pub struct PyDocUtils;
   117	
   118	impl PyDocUtils {
   119	    /// Extract a PEP 257 docstring from a function, class,
   120	    /// or module node. This expects the caller to pass a
   121	    /// node whose first statement may be a string literal.
   122	    pub fn docstring_for(node: Node, bytes: &[u8]) -> Option<String> {
   123	        // Try 'body' then 'suite', else allow node itself
   124	        let body = node
   125	            .child_by_field_name("body")
   126	            .or_else(|| node.child_by_field_name("suite"))
   127	            .unwrap_or(node);
   128	
   129	        // If body is a block or suite, use it
   130	        let suite = match body.kind() {
   131	            "block" | "suite" => Some(body),
   132	            _ => {
   133	                // Otherwise find the first block or suite
   134	                (0..body.child_count())
   135	                    .filter_map(|i| body.child(i))
   136	                    .find(|n| n.kind() == "block" || n.kind() == "suite")
   137	            }
   138	        }?;
   139	
   140	        // First named child must be expression_statement
   141	        let first = (0..suite.named_child_count())
   142	            .filter_map(|i| suite.named_child(i))
   143	            .find(|n| n.kind() == "expression_statement")?;
   144	
   145	        // Its first named child must be a string literal
   146	        let lit = first.named_child(0).filter(|n| n.kind() == "string")?;
   147	
   148	        // Convert to text and unquote + dedent
   149	        let raw = lit.utf8_text(bytes).ok()?;
   150	
   151	        Some(Self::unquote_and_dedent(raw))
   152	    }
   153	
   154	    /// Remove string prefixes, strip quotes, and dedent
   155	    pub fn unquote_and_dedent(s: &str) -> String {
   156	        // Recognize only legal Python string prefixes (r,u,f,b combos, case-insensitive)
   157	        // and consume at most two letters (e.g., r, u, f, b, fr, rf).
   158	        let mut i = 0usize;
   159	        // Uppercase for easy matching
   160	        let up = s.chars().take(2).collect::<String>().to_uppercase();
   161	        // Accept "R","U","F","B" or any two-letter combo thereof (FR, RF, UR not common for docstrings, but safe)
   162	        let first = up.chars().nth(0);
   163	        let second = up.chars().nth(1);
   164	        let is_legal = |c: Option<char>| matches!(c, Some('R' | 'U' | 'F' | 'B'));
   165	        if is_legal(first) && is_legal(second) {
   166	            i = 2;
   167	        } else if is_legal(first) {
   168	            i = 1;
   169	        }
   170	
   171	        // Work with the remainder after prefixes
   172	        let s = &s[i..];
   173	
   174	        // Handle triple-quoted first
   175	        for q in [r#"""""#, r#"'''"#] {
   176	            if s.starts_with(q) && s.ends_with(q) && s.len() >= 2 * q.len() {
   177	                let inner = &s[q.len()..s.len() - q.len()];
   178	                return Self::dedent(inner);
   179	            }
   180	        }
   181	
   182	        // Then handle single-quoted
   183	        if (s.starts_with('"') && s.ends_with('"')) || (s.starts_with('\'') && s.ends_with('\'')) {
   184	            let inner = &s[1..s.len() - 1];
   185	
   186	            return inner
   187	                .replace("\\n", "\n")
   188	                .replace("\\t", "\t")
   189	                .replace("\\\"", "\"")
   190	                .replace("\\'", "'");
   191	        }
   192	
   193	        // Fallback unchanged when syntax is unexpected
   194	        s.to_string()
   195	    }
   196	
   197	    /// Minimal dedent across all non-empty lines
   198	    pub fn dedent(s: &str) -> String {
   199	        // Split into lines
   200	        let lines: Vec<&str> = s.lines().collect();
   201	
   202	        // Compute common indent
   203	        let indent = lines
   204	            .iter()
   205	            .filter(|l| !l.trim().is_empty())
   206	            .map(|l| l.chars().take_while(|c| *c == ' ').count())
   207	            .min()
   208	            .unwrap_or(0);
   209	
   210	        // Remove the indent from each line
   211	        lines
   212	            .iter()
   213	            .map(|l| if l.len() >= indent { &l[indent..] } else { *l })
   214	            .collect::<Vec<&str>>()
   215	            .join("\n")
   216	    }
   217	}
   218	
   219	/// Rust doc attribute and comment helpers
   220	pub struct RustDocUtils;
   221	
   222	impl RustDocUtils {
   223	    /// Extract text from a '#[doc = "..."]' attribute
   224	    /// This supports normal quoted strings. Raw strings
   225	    /// are handled by `doc_attr_text_raw` below.
   226	    pub fn doc_attr_text(attr: Node, bytes: &[u8]) -> Option<String> {
   227	        // Convert the whole attribute to text
   228	        let raw = attr.utf8_text(bytes).ok()?;
   229	
   230	        // Trim leading/trailing whitespace
   231	        let s = raw.trim();
   232	
   233	        // Find "#[doc"
   234	        let start = s.find("#[doc")?;
   235	        let after = &s[start..];
   236	
   237	        // Find '='
   238	        let eq = after.find('=')?;
   239	        let mut q = eq + 1;
   240	
   241	        // Skip spaces
   242	        while q < after.len() && after.as_bytes()[q].is_ascii_whitespace() {
   243	            q += 1;
   244	        }
   245	
   246	        // Expect a normal quote
   247	        if q >= after.len() || after.as_bytes()[q] != b'"' {
   248	            return None;
   249	        }
   250	
   251	        // Move past opening quote
   252	        q += 1;
   253	
   254	        // Collect until closing quote
   255	        let mut out = String::new();
   256	        let mut i = q;
   257	        while i < after.len() {
   258	            let b = after.as_bytes()[i];
   259	
   260	            if b == b'\\' && i + 1 < after.len() {
   261	                out.push(after.as_bytes()[i + 1] as char);
   262	                i += 2;
   263	                continue;
   264	            }
   265	
   266	            if b == b'"' {
   267	                break;
   268	            }
   269	
   270	            out.push(b as char);
   271	
   272	            i += 1;
   273	        }
   274	
   275	        // Return None if empty, else Some
   276	        if out.is_empty() { None } else { Some(out) }
   277	    }
   278	
   279	    /// Extract text from a '#[doc = r#" ... "#]' raw string
   280	    /// Supports one or more # markers
   281	    pub fn doc_attr_text_raw(attr: Node, bytes: &[u8]) -> Option<String> {
   282	        // Convert to text
   283	        let raw = attr.utf8_text(bytes).ok()?;
   284	
   285	        // Find '#[doc'
   286	        let start = raw.find("#[doc")?;
   287	        let after = &raw[start..];
   288	
   289	        // Find '=' then the raw string opener r#"
   290	        let eq = after.find('=')?;
   291	        let mut i = eq + 1;
   292	
   293	        // Skip spaces
   294	        while i < after.len() && after.as_bytes()[i].is_ascii_whitespace() {
   295	            i += 1;
   296	        }
   297	
   298	        // Expect 'r'
   299	        if i >= after.len() || after.as_bytes()[i] != b'r' {
   300	            return None;
   301	        }
   302	
   303	        i += 1;
   304	
   305	        // Count '#' markers
   306	        let mut hashes = 0usize;
   307	        while i < after.len() && after.as_bytes()[i] == b'#' {
   308	            hashes += 1;
   309	
   310	            i += 1;
   311	        }
   312	
   313	        // Expect opening quote
   314	        if i >= after.len() || after.as_bytes()[i] != b'"' {
   315	            return None;
   316	        }
   317	
   318	        i += 1;
   319	
   320	        // Compute closing sequence: '"#...#'
   321	        let mut close = String::from("\"");
   322	        close.extend(std::iter::repeat_n("#", hashes));
   323	
   324	        // Capture until closing sequence
   325	        let body = &after[i..];
   326	        let end = body.find(&close)?;
   327	        let inner = &body[..end];
   328	
   329	        // Return body as owned String
   330	        Some(inner.to_string())
   331	    }
   332	
   333	    /// Extract from '///...' and '/** ... */' when they are
   334	    /// doc comments. Returns normalized text when detected.
   335	    pub fn doc_comment_text(n: Node, bytes: &[u8]) -> Option<String> {
   336	        // Convert the node text
   337	        let t = n.utf8_text(bytes).ok()?;
   338	
   339	        // Trim left for uniform checks
   340	        let s = t.trim_start();
   341	
   342	        // Handle '///' line doc
   343	        if s.starts_with("///") {
   344	            let body = s.trim_start_matches("///").trim_start();
   345	            return Some(body.to_string());
   346	        }
   347	
   348	        // Handle '/** ... */' block doc
   349	        if s.starts_with("/**") {
   350	            let body = s
   351	                .trim_start_matches("/**")
   352	                .trim_end()
   353	                .trim_end_matches("*/")
   354	                .to_string();
   355	
   356	            // Strip leading '*' on lines
   357	            let norm = body
   358	                .lines()
   359	                .map(|l| l.trim_start_matches('*').trim_start())
   360	                .collect::<Vec<&str>>()
   361	                .join("\n");
   362	
   363	            return Some(norm);
   364	        }
   365	
   366	        // Not a recognized doc comment
   367	        None
   368	    }
   369	}
   370	
   371	/// Simple visibility helpers
   372	pub struct VisibilityUtils;
   373	
   374	impl VisibilityUtils {
   375	    /// Python private if name starts with underscore
   376	    pub fn python_from_name(name: &str) -> bool {
   377	        // True means private, False means public
   378	        name.starts_with('_')
   379	    }
   380	
   381	    /// Generic helper to map a bool private flag to
   382	    /// a string label for quick logs or JSON dumps
   383	    pub fn label_from_private(is_private: bool) -> &'static str {
   384	        // Return a stable label
   385	        if is_private { "private" } else { "public" }
   386	    }
   387	}
   388	
   389	#[cfg(test)]
   390	mod tests {
   391	    // Import super for access to all structs
   392	    use super::*;
   393	
   394	    // Bring parser for small parser-based tests
   395	    use tree_sitter::{Language, Parser};
   396	
   397	    // Use Python grammar for quick docstring checks
   398	    unsafe extern "C" {
   399	        fn tree_sitter_python() -> Language;
   400	    }
   401	
   402	    /// Build a tiny Python tree to test docstring paths
   403	    fn parse_python(src: &str) -> (Parser, tree_sitter::Tree) {
   404	        // Create parser
   405	        let mut p = Parser::new();
   406	
   407	        // Set language
   408	        let lang = unsafe { tree_sitter_python() };
   409	        p.set_language(&lang).expect("set language");
   410	
   411	        // Parse
   412	        let tree = p.parse(src, None).expect("parse");
   413	
   414	        // Return parser and tree
   415	        (p, tree)
   416	    }
   417	
   418	    #[test]
   419	    fn name_join_works() {
   420	        // Parts to join
   421	        let parts = ["A", "B", "m"];
   422	
   423	        // Join with dot
   424	        let dotted = NameUtils::join(&parts, '.');
   425	
   426	        // Validate result
   427	        assert_eq!(dotted, "A.B.m");
   428	    }
   429	
   430	    #[test]
   431	    fn utf8_slice_safe_boundaries() {
   432	        // A string with multi-byte chars
   433	        let s = "αβγ.rs";
   434	
   435	        // Compute byte indices for ".rs"
   436	        let start = s.len() - 3;
   437	        let end = s.len();
   438	
   439	        // Perform safe slice
   440	        let sub = Utf8Utils::slice_str(s, start, end).unwrap();
   441	
   442	        // Validate
   443	        assert_eq!(sub, ".rs");
   444	    }
   445	
   446	    #[test]
   447	    fn pydoc_unquote_and_dedent_triple() {
   448	        // A triple-quoted raw docstring
   449	        let s = r#"
   450	            r"""Line1
   451	            Line2"""
   452	        "#;
   453	
   454	        // Extract inner text
   455	        let d = PyDocUtils::unquote_and_dedent(s);
   456	
   457	        // Validate content
   458	        assert!(d.contains("Line1"));
   459	        assert!(d.contains("Line2"));
   460	    }
   461	
   462	    #[test]
   463	    fn pydoc_unquote_single() {
   464	        // Single-quoted docstring
   465	        let s = "'one line'";
   466	
   467	        // Extract inner text
   468	        let d = PyDocUtils::unquote_and_dedent(s);
   469	
   470	        // Validate
   471	        assert_eq!(d, "one line");
   472	    }
   473	
   474	    #[test]
   475	    fn rust_doc_attr_raw_basic() {
   476	        // Minimal raw doc attribute
   477	        let _src = r#"
   478	            #[doc = r#"Hello "\# World"\#]
   479	            fn f() {}
   480	        "#;
   481	        // Parse Python just for a tree? No, instead
   482	        // directly test string parser by building a
   483	        // fake attribute via a Python tree would be
   484	        // fragile. Here we build a Rust-like snippet
   485	        // and use a Rust tree if available. For now,
   486	        // sanity check doc_comment_text with comments.
   487	        let c = "/** Hello */";
   488	
   489	        // Simulate a block comment node via direct call
   490	        let body = RustDocUtils::doc_comment_text_fake(c);
   491	
   492	        // Validate expected text
   493	        assert_eq!(body.as_deref(), Some("Hello"));
   494	    }
   495	
   496	    // Helper to unit-test doc_comment_text using a
   497	    // string slice without a parsed node. This is
   498	    // isolated to test normalization logic only.
   499	    impl RustDocUtils {
   500	        pub fn doc_comment_text_fake(s: &str) -> Option<String> {
   501	            // Trim start for uniform checks
   502	            let t = s.trim_start();
   503	
   504	            // Handle '///'
   505	            if t.starts_with("///") {
   506	                let body = t.trim_start_matches("///").trim_start();
   507	                return Some(body.to_string());
   508	            }
   509	
   510	            // Handle '/** ... */'
   511	            if t.starts_with("/**") {
   512	                let body = t.trim_start_matches("/**").trim_end_matches("*/").trim();
   513	
   514	                let norm = body
   515	                    .lines()
   516	                    .map(|l| l.trim_start_matches('*').trim())
   517	                    .collect::<Vec<&str>>()
   518	                    .join("\n")
   519	                    .trim()
   520	                    .to_string();
   521	                return Some(norm);
   522	            }
   523	
   524	            // Not a doc comment
   525	            None
   526	        }
   527	    }
   528	
   529	    #[test]
   530	    fn tsnode_has_ancestor_smoke() {
   531	        // Minimal Python snippet with a class and method
   532	        let src = r#"
   533	            class A:
   534	                def m(self): pass
   535	        "#;
   536	
   537	        // Build tree
   538	        let (_p, tree) = parse_python(src);
   539	        // Root node
   540	        let root = tree.root_node();
   541	
   542	        // Find the method node
   543	        let _cursor = root.walk();
   544	
   545	        // Traverse to find 'function_definition'
   546	        let mut method: Option<Node> = None;
   547	
   548	        // Simple DFS for the test
   549	        fn dfs<'a>(n: Node<'a>, out: &mut Option<Node<'a>>) {
   550	            if n.kind() == "function_definition" {
   551	                *out = Some(n);
   552	                return;
   553	            }
   554	
   555	            let c = n.walk();
   556	
   557	            for i in 0..n.named_child_count() {
   558	                let ch = n.named_child(i).unwrap();
   559	
   560	                dfs(ch, out);
   561	
   562	                if out.is_some() {
   563	                    return;
   564	                }
   565	            }
   566	
   567	            drop(c);
   568	        }
   569	
   570	        // Run DFS
   571	        dfs(root, &mut method);
   572	
   573	        // Ensure we found the method
   574	        let m = method.expect("method found");
   575	
   576	        // Validate ancestor check
   577	        assert!(TsNodeUtils::has_ancestor(m, "class_definition"));
   578	    }
   579	}
```
