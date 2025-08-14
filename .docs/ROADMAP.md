# roughup Development Roadmap

**Vision**: Transform roughup into the definitive CLI for GPT-5 web chat workflows, delivering surgical, deterministic code context at scale.

**Core Philosophy**: Single-purpose excellence - optimize for LLM consumption while preserving semantic precision and developer ergonomics.

---

## Strategic Objectives

### 🎯 **Primary Goals**
- **Chat-first design**: Every output optimized for web chat paste workflows
- **Token budget awareness**: Never exceed chat limits while maximizing semantic value
- **Deterministic reproducibility**: Same input → same output, always
- **Delta efficiency**: Follow-ups send only what changed
- **Precise provenance**: Every snippet traceable to exact source location

### 🔬 **Quality Metrics**
- **Time-to-paste**: <30s from command to chat-ready output
- **Context efficiency**: >80% of budget spent on relevant code
- **Follow-up speed**: <5s for incremental updates
- **Reproducibility**: 100% identical output across runs

---

## Phase 1: Chat-First CLI Foundation (Q3 2025)

### 1.1 Core Chat Commands
**Deliverable**: 5 new chat-optimized CLI surfaces

#### `rup chatpack` - Single-File Context Bundles
```bash
rup chatpack --budget 30k --focus "src/**/*.rs" --preserve-docstrings
```

**Outputs**:
- `CHATPACK.md`: Self-contained, paste-ready context bundle
  - Auto-generated "Question & Context" header (10-15 lines)
  - Compact repo tree (depth 2-3) with metadata
  - Symbol summary with counts by file/kind
  - Fenced code blocks with language tags and path/line headers
- `manifest.json`: Machine-readable generation metadata

**Key Flags**:
- `--budget <tokens|chars>`: Hard budget limits
- `--focus <glob|regex>`: Target specific paths/patterns  
- `--max-files N`: Cap file inclusion
- `--langs rust,python`: Language filtering
- `--preserve-docstrings`: Keep documentation
- `--no-comments`: Strip non-doc comments

#### `rup followup` - Delta-Only Updates
```bash
rup followup --since audit/manifest.json --budget 15k
```
- Detects changed files/symbols since last manifest
- Re-extracts only impacted spans
- Generates minimal "Update Summary" blocks
- **Target**: <5s execution for typical changes

#### `rup diffpack` - Review-Ready Comparisons
```bash
rup diffpack --from HEAD~1 --to HEAD --only rust,python
```
- Unified diffs plus symbol-aligned context
- Optimized for design/code review workflows
- Cross-references symbol changes with usage

#### `rup focus` - Failure-Driven Extraction
```bash
cargo test 2> fail.txt && rup focus --from fail.txt
```
- Parses Rust backtraces (`fn at path:line`)
- Extracts Python tracebacks with context
- Maps test failures to relevant symbols
- Includes ±N lines of contextual code

#### `rup select` - Surgical Symbol Selection
```bash
rup select --symbols 'Module::Type::method, re:^parse_.*' --neighbors 2
```
- Precise symbol/file targeting
- Callsite discovery via Tree-sitter analysis
- Multi-level caller/callee inclusion
- Pattern-based symbol matching

**Success Criteria**:
- All 5 commands implemented and tested
- Integration test suite covering common workflows
- Documentation with example usage patterns
- Performance benchmarks established

### 1.2 Chat-Optimized Output Formats
**Deliverable**: Purpose-built output ergonomics

#### Smart Budget Management
```bash
rup chatpack --chat-mode balanced  # ~30k chars, optimized chunking
```
- **tight**: ~15k chars, essential code only
- **balanced**: ~30k chars, includes context
- **verbose**: ~60k chars, comprehensive coverage
- Hard caps per snippet (6-10k chars) prevent truncation
- Automatic pagination with "Part k/N" headers

#### Stable Content Addressing
Every code block gets referenceable headers:
```markdown
// path: src/core/chunk.rs | lines: 120–220 | CID: 7d2e3a1
```
- `CID`: Stable content hash for cross-references
- Enables precise snippet discussions in chat
- Supports automated linking and updates

#### Provenance Footer
```markdown
---
Generated: 2025-08-14T19:40:00Z | Commit: abc123ef | Command: rup chatpack --budget 30k
Manifest Hash: def456ab | Version: roughup 0.2.0
```
- Complete reproducibility information
- Audit trail for generated content
- Version compatibility tracking

#### Content Minimization
- `--redact-secrets`: Automatic PII/key detection
- `--strip-comments`: Remove non-essential comments
- `--only-public`: Public API surfaces only
- Smart whitespace normalization

**Success Criteria**:
- Chat message size never exceeds platform limits
- Content hash stability across identical inputs
- <10% budget waste on formatting overhead
- Zero information loss during minimization

---

## Phase 2: Language-Specific Excellence (Q4 2025)

### 2.1 Rust Symbol Fidelity Upgrades
**Deliverable**: Production-grade Rust analysis

#### Enhanced Symbol Classification
- `async fn` detection and labeling
- Trait method vs inherent impl distinction
- Macro invocation tracking and expansion points
- Generic parameter and lifetime analysis

#### Implementation Coalescing
```bash
rup chatpack --impl-coalesce --budget 25k
```
- Merge scattered `impl` blocks for same type
- Preserve individual method signatures
- Maintain logical code organization
- Cross-reference related implementations

#### Advanced Context Extraction
- Associated type and constant inclusion
- Trait bound analysis and documentation
- Macro definition to usage linking
- Module-level documentation preservation

### 2.2 Python Precision Enhancements
**Deliverable**: PEP-compliant Python processing

#### Docstring and Decorator Handling
- PEP 257 triple-quoted docstring preservation
- Decorator normalization in headers (`@staticmethod`, `@lru_cache`)
- Class hierarchy and inheritance tracking
- Method resolution order awareness

#### Method Context Expansion
```bash
rup select --method-context --symbols "MyClass.method"
```
- Automatic class header inclusion
- `__init__` signature when selecting methods
- Instance variable discovery and typing
- Related method suggestions

#### Import and Dependency Analysis
- Qualified reference tracking (`mod.func`, `self.method`)
- Cross-module usage patterns
- Circular dependency detection
- Unused import identification

### 2.3 Symbol Neighborhood Discovery
**Deliverable**: Intelligent code relationship mapping

#### Rust Callsite Analysis
```bash
rup select --refs callers --refs-limit 5 --symbols "parse_config"
```
- Tree-sitter query-based callsite discovery
- Function usage pattern analysis
- Macro expansion point tracking
- Generic instantiation mapping

#### Python Reference Tracing
- Qualified name resolution across modules
- Method call chain reconstruction
- Class hierarchy usage patterns
- Dynamic attribute access detection

**Success Criteria**:
- 95%+ symbol classification accuracy
- <500ms processing time for typical files
- Zero false positives in callsite discovery
- Complete docstring preservation fidelity

---

## Phase 3: Reproducible Workflow System (Q1 2026)

### 3.1 Durable Manifest Schema
**Deliverable**: Version-controlled generation metadata

#### Core Schema Definition
```json
{
  "version": 1,
  "created_at": "2025-08-14T19:40:00Z",
  "repo": { 
    "root": "/abs/path", 
    "commit": "abc123",
    "branch": "main",
    "dirty": false 
  },
  "budget": { 
    "mode": "balanced", 
    "char_limit": 30000,
    "token_estimate": 8500 
  },
  "filters": { 
    "langs": ["rust","python"], 
    "files": ["src/**"], 
    "symbols": ["Module::*"],
    "exclude": ["**/tests/**"]
  },
  "artifacts": [
    {
      "cid": "7d2e3a1",
      "path": "src/core/chunk.rs",
      "language": "rust",
      "start_line": 120,
      "end_line": 220,
      "chars": 5821,
      "symbols": ["ChunkProcessor::new", "ChunkProcessor::process"],
      "last_modified": "2025-08-14T19:35:00Z"
    }
  ],
  "generation_stats": {
    "files_scanned": 45,
    "symbols_found": 234,
    "chars_included": 28450,
    "budget_utilization": 0.948,
    "duration_ms": 1250
  }
}
```

#### Manifest-Driven Operations
- `followup` command powered by artifact comparison
- Regenerate identical outputs from manifest
- Version migration support for schema evolution
- Cross-platform path normalization

### 3.2 Advanced Budgeting System
**Deliverable**: GPT-5 token-aware resource management

#### Multi-Model Token Estimation
```bash
rup chatpack --model gpt-4o --budget tokens:8000
```
- Accurate token counting per model family
- Character-to-token conversion tables
- Context window utilization optimization
- Overflow prevention with graceful degradation

#### Content Prioritization
- Symbol importance scoring (usage frequency, public API)
- Dependency graph traversal for inclusion decisions
- Critical path analysis for bug-related extracts
- User-defined priority hints and overrides

#### Budget Visualization
```bash
rup chatpack --dry-run --budget 30k
```
**Output**:
```
Budget Analysis (30,000 chars target):
  Candidates: 67 symbols across 23 files
  Priority Queue:
    HIGH  │ src/core/parser.rs    │ 2,340 chars │ 5 symbols │ ✓ Include
    HIGH  │ src/api/endpoints.rs  │ 1,890 chars │ 3 symbols │ ✓ Include
    MED   │ src/utils/helpers.rs  │ 1,200 chars │ 8 symbols │ ✓ Include
    LOW   │ tests/integration.rs  │   950 chars │ 2 symbols │ ✗ Exclude
  
  Final Selection: 28,450 chars (94.8% utilization)
  Excluded: 15 symbols (budget constraints)
```

**Success Criteria**:
- Schema stability across versions
- <2% variance in token estimates vs actual
- 100% reproducible outputs from manifests
- Sub-second manifest generation and comparison

---

## Phase 4: Performance and Developer Experience (Q2 2026)

### 4.1 Intelligent Caching System
**Deliverable**: Sub-second incremental operations

#### Multi-Layer Caching Strategy
- **L1**: In-memory symbol AST cache (per-session)
- **L2**: Disk cache for symbol extraction (per-file mtime)
- **L3**: Project-wide symbol relationship cache
- Cache invalidation based on file modification times

#### Cache Key Design
```
cache_key = hash(file_path, file_size, mtime, parser_version, extraction_config)
```
- Automatic invalidation on Tree-sitter updates
- Configuration-aware cache partitioning
- Cross-platform path normalization
- Garbage collection for stale entries

#### Performance Targets
- **Cold start**: <3s for medium repositories (1000 files)
- **Warm cache**: <500ms for incremental updates
- **Memory usage**: <50MB cache overhead for large projects
- **Cache hit rate**: >85% for typical development workflows

### 4.2 Quality Assurance Framework
**Deliverable**: Comprehensive testing and validation

#### Conformance Test Suite
- **Rust fixtures**: All language constructs and edge cases
- **Python fixtures**: PEP compliance and dialect coverage
- **Cross-platform validation**: Windows, macOS, Linux
- **Tree-sitter version compatibility**: Guard against grammar drift

#### Automated Benchmarking
```bash
make bench
```
**Metrics Tracked**:
- Processing speed (files/second, symbols/second)
- Memory efficiency (peak usage, allocation patterns)
- Cache performance (hit rates, invalidation frequency)
- Output quality (symbol coverage, false positive rates)

#### Regression Detection
- Golden file comparison for output stability
- Performance regression alerts (>10% slowdown)
- Symbol extraction accuracy validation
- Budget utilization efficiency tracking

### 4.3 Pre-Built Workflow Profiles
**Deliverable**: One-command solutions for common scenarios

#### Profile Implementations
```bash
# Bug investigation workflow
rup chatpack --profile bugfix --from-backtrace fail.txt
# → Focus on failure points, include ±20 lines, add callers

# API design review
rup chatpack --profile design-review --budget 25k
# → Public surfaces only, preserve docstrings, balanced budget

# Performance analysis
rup chatpack --profile perf-dive --hot-files 10
# → Size-based prioritization, allocation-heavy code, utility bloat detection

# Security audit
rup chatpack --profile security --focus "auth/**,crypto/**"
# → Sensitive code paths, input validation, crypto usage patterns

# Onboarding guide
rup chatpack --profile onboarding --entry-points main,lib
# → Module structure, public APIs, example usage patterns
```

#### Profile Configuration
- TOML-based profile definitions in `~/.config/roughup/profiles/`
- Organization-wide profile sharing via git repositories
- Template system for custom profile creation
- Profile composition and inheritance

**Success Criteria**:
- 90%+ cache hit rate after initial warmup
- Complete test coverage for language features
- <5 minute setup time for new developers
- Zero-configuration profiles for 80% of use cases

---

## Phase 5: Ecosystem Integration (Q3-Q4 2026)

### 5.1 Development Tool Integrations

#### IDE Extensions
- **VS Code Extension**: Right-click → Generate Chat Context
- **JetBrains Plugin**: Symbol selection → roughup extraction
- **Vim Plugin**: Range selection and buffer integration
- **Emacs Package**: org-mode integration for documentation

#### CI/CD Pipeline Integration
```yaml
# GitHub Actions
- name: Generate PR Context
  uses: roughup-action@v1
  with:
    profile: 'design-review'
    budget: '25k'
    output: 'pr-context.md'
```

#### Git Hooks Integration
```bash
# Pre-commit hook for context generation
rup diffpack --from HEAD~1 --to HEAD > .git/review-context.md
```

### 5.2 API and Service Modes

#### HTTP API Server
```bash
rup serve --port 8080 --cors-allow "*"
```
**Endpoints**:
- `POST /chatpack` - Generate context bundles
- `POST /followup` - Incremental updates  
- `GET /manifest/:id` - Retrieve generation metadata
- `WS /watch` - Real-time file system monitoring

#### Language Server Protocol
- Semantic symbol information for editors
- Real-time context generation as you code
- Cross-reference and callsite discovery
- Integration with existing LSP clients

### 5.3 Cloud and Distribution

#### Container Images
```dockerfile
FROM alpine:latest
COPY roughup /usr/local/bin/
ENTRYPOINT ["roughup"]
```
- Multi-architecture support (amd64, arm64)
- Minimal attack surface with distroless base
- Pre-built Tree-sitter grammars included
- Volume mounting for workspace access

#### Package Distribution
- **Homebrew formula** for macOS users
- **Chocolatey package** for Windows users  
- **Debian/Ubuntu packages** via apt repository
- **Docker Hub automated builds** with version tags

**Success Criteria**:
- IDE extensions with >1000 active users
- API response times <200ms for typical requests
- 99.9% uptime for hosted services
- Zero-friction installation across all platforms

---

## Implementation Timeline

### Q3 2025: Foundation Sprint
**Duration**: 12 weeks
- ✅ **Weeks 1-4**: Core chatpack command and basic output formats
- ✅ **Weeks 5-8**: Followup and focus commands with manifest system  
- ✅ **Weeks 9-12**: Budget management and content addressing

### Q4 2025: Language Excellence
**Duration**: 12 weeks  
- 🔄 **Weeks 1-6**: Rust symbol fidelity upgrades and impl coalescing
- 📋 **Weeks 7-12**: Python precision enhancements and neighborhood discovery

### Q1 2026: Workflow Maturity
**Duration**: 12 weeks
- 📋 **Weeks 1-6**: Manifest schema finalization and budget system
- 📋 **Weeks 7-12**: Reproducibility guarantees and cross-platform testing

### Q2 2026: Performance Optimization  
**Duration**: 12 weeks
- 📋 **Weeks 1-6**: Caching system implementation and benchmarking
- 📋 **Weeks 7-12**: Quality assurance framework and profile system

### Q3-Q4 2026: Ecosystem Expansion
**Duration**: 24 weeks
- 📋 **Weeks 1-12**: IDE integrations and CI/CD tooling
- 📋 **Weeks 13-24**: API services and cloud distribution

---

## Success Metrics and KPIs

### User Experience Metrics
- **Time-to-Value**: <60 seconds from install to first useful output
- **Learning Curve**: <10 minutes to master basic workflows
- **Error Recovery**: <5% of operations require manual intervention
- **User Retention**: >80% monthly active user retention

### Technical Performance Metrics  
- **Processing Speed**: >1000 files/second on modern hardware
- **Memory Efficiency**: <100MB peak usage for large repositories
- **Cache Hit Rate**: >90% for incremental operations
- **Output Accuracy**: >99% symbol classification precision

### Ecosystem Adoption Metrics
- **GitHub Stars**: >5000 (quality/utility indicator)
- **Package Downloads**: >10k monthly (adoption velocity)
- **Integration Usage**: >50 public projects using roughup in CI
- **Community Contributions**: >20 external contributors

### Business Impact Metrics
- **Developer Productivity**: 25% reduction in context-gathering time
- **Code Review Quality**: 30% improvement in review thoroughness
- **Bug Resolution Speed**: 20% faster average time-to-fix
- **Documentation Coverage**: 40% increase in API documentation

---

## Risk Mitigation Strategies

### Technical Risks
| Risk | Impact | Probability | Mitigation |
|------|--------|-------------|------------|
| Tree-sitter grammar breaking changes | High | Medium | Version pinning, compatibility testing, fallback parsers |
| Performance degradation at scale | High | Low | Continuous benchmarking, optimization feedback loops |
| Cross-platform compatibility issues | Medium | Medium | Multi-OS CI, platform-specific testing, user feedback |
| Token estimation accuracy drift | Medium | High | Model-specific calibration, estimation algorithm updates |

### Product Risks
| Risk | Impact | Probability | Mitigation |
|------|--------|-------------|------------|
| User adoption slower than expected | Medium | Medium | Enhanced documentation, example workflows, community outreach |
| Competitor emergence | Low | High | Feature differentiation, performance leadership, user lock-in |
| Changing LLM landscape | High | High | Model-agnostic design, adapter pattern, format flexibility |
| Maintenance burden growth | Medium | High | Automated testing, code quality gates, contributor onboarding |

### Organizational Risks
| Risk | Impact | Probability | Mitigation |
|------|--------|-------------|------------|
| Resource constraints | High | Low | Phased delivery, MVP prioritization, community contributions |
| Scope creep beyond core mission | Medium | Medium | Clear product vision, feature evaluation criteria, user feedback |
| Technical debt accumulation | Medium | Medium | Regular refactoring sprints, code review standards, architecture reviews |

---

## Community and Contribution Strategy

### Open Source Governance
- **MIT License**: Maximum adoption and commercial use freedom
- **Code of Conduct**: Welcoming, inclusive development environment  
- **Contributor Guidelines**: Clear onboarding path and expectations
- **Architecture Decision Records**: Transparent technical decision making

### Community Building Initiatives
- **Monthly dev streams**: Live coding sessions and Q&A
- **Plugin ecosystem**: Third-party language parser support
- **Example repositories**: Showcase workflows and best practices
- **User showcase**: Highlight creative usage patterns

### Documentation Strategy
- **Interactive tutorial**: Learn-by-doing with real repositories
- **API reference**: Complete technical documentation  
- **Video walkthroughs**: Visual learning for complex workflows
- **Community wiki**: User-contributed tips and tricks

---

## Long-Term Vision (2027+)

### Expanded Language Ecosystem
- **Universal parser support**: Any Tree-sitter grammar
- **Custom DSL handling**: Configuration files, data formats
- **Cross-language analysis**: Call graphs spanning multiple languages
- **Semantic understanding**: Beyond syntax to program meaning

### AI-Native Features  
- **Intent recognition**: Natural language to roughup commands
- **Automatic context curation**: ML-driven relevance scoring
- **Predictive extraction**: Anticipate needed context for tasks
- **Cross-repository insights**: Patterns across project portfolios

### Enterprise Integration
- **SAML/SSO authentication**: Enterprise security compliance
- **Audit logging**: Complete operation traceability
- **Role-based access**: Fine-grained permission controls  
- **Custom deployment**: On-premises and air-gapped environments

### Research and Innovation
- **Program analysis research**: Advanced static analysis techniques
- **LLM optimization**: Context format research and development
- **Developer productivity studies**: Quantified impact measurement
- **Open source ecosystem**: Tool interoperability standards

---

*This roadmap represents a living document that will evolve based on user feedback, technical discoveries, and ecosystem changes. Regular quarterly reviews ensure alignment with strategic objectives while maintaining development velocity and quality standards.*