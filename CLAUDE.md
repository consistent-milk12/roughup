# CLAUDE.md - Roughup Quality Protocol

## Core Rules: Quality Over Velocity

**Pre-Implementation:** Ask "why/where" before implementing | Use lib.rs + surgical reads | Respect SLAs (<2s context, <300ms rollback)
**Anti-Patterns:** No mod.rs/Glob discovery | No full-file reads | No implementation without lib.rs consultation
**Constraints:** Atomic guarantees/BLAKE3/repo boundaries | Phase 3.5 architecture alignment

## Session Protocol

**Start:** Acknowledge CLAUDE.md + TODO.md surgical extraction + lib.rs → confirm phase
**Workflow:** `rup context --budget 4000 --template feature [keywords] --semantic` before implementing
**Mid-session:** "Quality-first?" every 5-7 exchanges | "PAUSE" → re-read rules

## Architecture Efficiency

**lib.rs:** Full read for new features | Targeted for existing patterns | Focus on module placement/API
**TODO.md Windows:** Current(82-107) | Next(108-150) | Latest(349-395) | Arch(283-298)
**Updates:** Session end only | Max 50 lines | ✅ status | Preserve structure

## Token Efficiency

**Response:** Direct action | Batch tool calls | Code-first demos | Minimal acknowledgments
**Input:** Batch reads | <100 lines justified | grep/offset patterns | `rup context` not file hunting

## Quality Gates

**Test:** `cargo check` before edits | Run affected tests | Full suite for critical
**Perf:** Benchmark hot paths | Verify SLA | Profile allocation-heavy
**Error:** Result/Option | Handle edges | No unwrap() in production
**Commit:** Tests + clippy + perf verified

**Insights:**

```
★ Insight ─────────────────────────────────────
[2-3 key points specific to implementation]
─────────────────────────────────────────────────
```

---

# Mentor Handoff Protocol

## Goal: `MENTOR_PACKET.md` → Fast Review → Patch → Validate

## Packet Structure (10 sections)

1. **YAML:** task/phase/reason/constraints/cli-cmd
2. **Brief:** Problem/success/non-goals/interfaces/risks (≤10 lines)
3. **Arch:** lib.rs modules touched + why (≤30 lines)
4. **TODO.md:** Windows 82-107, 108-150, 349-395, 283-298 only
5. **Evidence:** ≤4 spans, ≤120 lines each, exact anchors known
6. **Behavior:** Last working CLI + DCR/CEF/TVE or N/A
7. **Tests:** Unit/integration with file::case names + Given/When/Then
8. **Anchors:** Exact patterns for patch landing
9. **Constraints:** MSRV/determinism/perf/no-glob (≤10)
10. **Deliverables:** [ ] diff [ ] tests + open questions

## Protocol

**A. Start:** "Acknowledged. Phase=X. Pulling lib.rs + TODO windows + ≤4 spans."
**B. Collect:** lib.rs→arch | TODO windows→context | known anchors→evidence | user ask→brief/tests
**C. Gates:** No span >120 | ≤6 total | anchors present | tests named | no glob/mod.rs
**D. Handoff:** "Mentor packet ready." + file only
**E. Apply:** diff→check/test/clippy→pass/fail + BUGREPORT.md if red

## Templates

**MENTOR_PACKET.md:**

```yaml
task: "X"
phase: "Phase 3.5/Week Y"
reason: "Z impact"
repo: { branch: "X", commit: "abc123" }
constraints: { slas: ["context <2s"], invariants: ["atomic"] }
cli: { suggested_context_cmd: "rup context --budget 4000..." }
```

Brief: Problem/Success/Non-goals/Interfaces/Risks
Arch: core::X(why), cli::Y(why)  
TODO: L108-150 excerpt
Evidence: FILE spans with reasons
Tests: file::case names  
Anchors: "pub struct X" patterns
Constraints: MSRV/perf/determinism
Deliverables: [ ] diff [ ] tests

**BUGREPORT.md:** cmd/exit/stderr(≤60)/failed-tests/notes

## Checklist

- [ ] lib.rs arch map
- [ ] TODO windows only
- [ ] ≤4 spans ≤120 lines
- [ ] Named tests
- [ ] Exact anchors
- [ ] No glob/mod.rs
