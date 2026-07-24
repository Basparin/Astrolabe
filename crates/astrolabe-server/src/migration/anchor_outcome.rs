use super::*;

/// Envelope schema for the `anchor_outcome` MCP tool / `astrolabe cli
/// anchor_outcome` subcommand.
pub(crate) const ANCHOR_OUTCOME_SCHEMA: &str = "astrolabe.anchor_outcome.v1";
/// Ledger actor recorded for every anchor ingest driven by this surface.
pub(crate) const ANCHOR_OUTCOME_ACTOR: &str = "astrolabe-anchor-outcome";

/// Shared request path for the `anchor_outcome` MCP tool and its `astrolabe cli
/// anchor_outcome` subcommand.
///
/// Both surfaces funnel identical raw arguments through this one function, which
/// (1) enforces the shadow dial, (2) builds the validated outcome request via the
/// single `astrolabe_anchors::build_test_run_request` path, (3) resolves subject
/// ids to current constellation ids from the persisted vault node map, and (4)
/// ingests the anchors in one atomic Grounding-ledger group commit. Every stage
/// is fail-closed: an unknown outcome kind, malformed report/timestamp, a
/// source/confidence that violates the grounding invariants, or a conflicting
/// re-post each return a labeled `status: "refused"` envelope with its stable
/// `{code, message, remediation}` and no partial anchor is written. Because both
/// paths construct the request here from the same inputs, identical inputs
/// persist byte-identical anchor and ledger state by construction.
#[allow(clippy::too_many_arguments)]
pub(crate) fn anchor_outcome_json_at(
    cache_dir: &Path,
    project: &str,
    kind: &str,
    source: &str,
    confidence: Option<f32>,
    format: &str,
    report_text: &str,
    observed_at: &str,
) -> Result<Value, DynError> {
    if kind != "test_run" {
        return Ok(anchor_outcome_refused(
            project,
            source,
            "ASTRO_ANCHOR_KIND_UNSUPPORTED",
            format!(
                "anchor_outcome outcome kind {kind:?} is not available on this surface; only \
                 test_run is wired with a built-in payload"
            ),
            "pass kind=\"test_run\" with a format + report, or file an issue to wire the \
             agent_task/review/incident/manual_label payloads",
        ));
    }

    if read_dial_at(cache_dir, project)? != MigrationDial::Shadow {
        return Ok(anchor_outcome_refused(
            project,
            source,
            "ASTRO_ANCHOR_SHADOW_REQUIRED",
            format!("anchor_outcome requires calyx shadow indexing for project {project:?}"),
            "run index_repository with calyx=\"shadow\" for this project before anchoring outcomes",
        ));
    }

    let (vault_dir, vault_id, vault_salt) = shadow_vault_config_at(cache_dir, project)?;
    if !vault_dir.exists() {
        return Ok(anchor_outcome_refused(
            project,
            source,
            "ASTRO_ANCHOR_VAULT_MISSING",
            format!("shadow vault dir missing: {}", vault_dir.display()),
            "rerun index_repository with calyx=\"shadow\" before anchoring outcomes",
        ));
    }

    // Single shared request-construction path: identical inputs => byte-identical
    // OutcomeAnchorRequest on both the MCP and CLI surfaces by construction.
    let mut request = match astrolabe_anchors::build_test_run_request(
        source,
        observed_at,
        confidence,
        format,
        report_text,
    ) {
        Ok(request) => request,
        Err(error) => {
            return Ok(anchor_outcome_refused(
                project,
                source,
                error.code(),
                error.message().to_string(),
                error.remediation(),
            ));
        }
    };

    let vault = open_shadow_vault_writable(
        &vault_dir,
        &vault_id,
        &vault_salt,
        vec![
            ColumnFamily::Ledger,
            ColumnFamily::Anchors,
            ColumnFamily::Graph,
        ],
    )?;
    // Graph-based test-subject resolution (#3): a test-report case id
    // (`<test file>::<case title>`) never equals a node-map qualified name, so
    // resolve each case to the symbols its test file exercises — the file's
    // nodes, following their outgoing edges to Function/Method nodes outside
    // the file. Unresolvable cases are kept verbatim and surface as unmapped.
    let (resolution_nodes, resolution_edges) =
        astrolabe_ingest::read_subject_resolution_graph(&vault, project)?;
    let subject_index = SubjectGraphIndex::new(&resolution_nodes, &resolution_edges);
    let graph_resolution = subject_index.resolve_request(&mut request);

    // Subject -> CxId resolution against the persisted node map; unresolved
    // subjects are accounted (never a silent guess) by ingest_outcome_anchors.
    let cx_ids = astrolabe_ingest::read_node_map_cx_ids(&vault, project)?;
    let report = match astrolabe_anchors::ingest_outcome_anchors(
        &vault,
        &request,
        &cx_ids,
        ANCHOR_OUTCOME_ACTOR,
    ) {
        Ok(report) => report,
        Err(error) => {
            drop(vault);
            return Ok(anchor_outcome_refused(
                project,
                source,
                error.code,
                error.message,
                error.remediation,
            ));
        }
    };
    drop(vault);

    let status = if report.anchors_written > 0 {
        "grounded"
    } else {
        "noop"
    };
    let unmapped_count = report.unmapped_subjects.len();
    Ok(json!({
        "schema": ANCHOR_OUTCOME_SCHEMA,
        "project": project,
        "status": status,
        "outcome_kind": request.kind.as_str(),
        "source": request.source,
        "observed_at": request.observed_at,
        "confidence": request.confidence,
        "subjects_presented": request.subjects.len(),
        "anchors_written": report.anchors_written,
        "anchors_deduplicated": report.anchors_deduplicated,
        "rows_written": report.rows_written,
        "unmapped_subjects": report.unmapped_subjects,
        "unmapped_subject_count": unmapped_count,
        "graph_resolution": graph_resolution,
        "anchor_dump_hash": report.anchor_dump_hash,
        "ledger_ref": ledger_ref_json(&report.ledger_ref),
        "fsv": report.fsv.as_ref().map(fsv_ack_envelope),
        "grounding_delta": {
            "anchors_written": report.anchors_written,
            "rows_written": report.rows_written,
            "ledger_seq": report.ledger_ref.seq,
        },
        // Source trust is a grounding property, distinct from the verified FSV
        // witness above. Proxy evidence can never ride as Trusted.
        "trust": report.trust.as_str(),
        "freshness": "fresh",
        "provenance": [
            format!("ledger:grounding:seq={}", report.ledger_ref.seq),
            format!("anchor_dump_hash:{}", report.anchor_dump_hash),
            format!("source:{}", request.source),
        ],
    }))
}

/// Adjacency index over the persisted node map for graph-based resolution of
/// test-report case ids to tested symbols.
struct SubjectGraphIndex<'a> {
    /// project-relative file path -> CxIds of the nodes in that file.
    file_members: BTreeMap<&'a str, BTreeSet<calyx_core::CxId>>,
    /// CxId -> (label, qualified name).
    node_meta: BTreeMap<calyx_core::CxId, (&'a str, &'a str)>,
    /// src CxId -> outgoing edge targets.
    out_edges: BTreeMap<calyx_core::CxId, Vec<calyx_core::CxId>>,
}

impl<'a> SubjectGraphIndex<'a> {
    fn new(
        nodes: &'a [astrolabe_ingest::SubjectResolutionNode],
        edges: &'a [(calyx_core::CxId, calyx_core::CxId)],
    ) -> Self {
        let mut file_members: BTreeMap<&str, BTreeSet<calyx_core::CxId>> = BTreeMap::new();
        let mut node_meta = BTreeMap::new();
        for node in nodes {
            file_members
                .entry(node.file_path.as_str())
                .or_default()
                .insert(node.cx_id);
            node_meta.insert(
                node.cx_id,
                (node.label.as_str(), node.qualified_name.as_str()),
            );
        }
        let mut out_edges: BTreeMap<calyx_core::CxId, Vec<calyx_core::CxId>> = BTreeMap::new();
        for (src, dst) in edges {
            out_edges.entry(*src).or_default().push(*dst);
        }
        Self {
            file_members,
            node_meta,
            out_edges,
        }
    }

    /// Qualified names of the Function/Method nodes outside `case_id`'s test
    /// file that any node of that file points an edge at. `None` when the
    /// case id has no `::` separator, the file is not in the node map, or the
    /// file's nodes reach no external symbol.
    fn targets_for(&self, case_id: &str) -> Option<Vec<&'a str>> {
        let (raw_path, _title) = case_id.split_once("::")?;
        let norm = raw_path.replace('\\', "/");
        // Longest node-map file path that is a boundary-safe suffix of the
        // (absolute) report path; node-map paths are project-relative.
        let mut best: Option<&str> = None;
        for file in self.file_members.keys() {
            if !norm.ends_with(file) {
                continue;
            }
            // ends_with guarantees norm.len() >= file.len(); strict greater
            // means a preceding byte exists to boundary-check.
            let boundary_ok = norm.len() == file.len()
                || norm.as_bytes()[norm.len() - file.len() - 1] == b'/';
            if boundary_ok && best.is_none_or(|current| file.len() > current.len()) {
                best = Some(file);
            }
        }
        let members = self.file_members.get(best?)?;
        let mut targets = BTreeSet::new();
        for member in members {
            let Some(dsts) = self.out_edges.get(member) else {
                continue;
            };
            for dst in dsts {
                if members.contains(dst) {
                    continue;
                }
                if let Some((label, qualified_name)) = self.node_meta.get(dst)
                    && matches!(*label, "Function" | "Method")
                {
                    targets.insert(*qualified_name);
                }
            }
        }
        if targets.is_empty() {
            None
        } else {
            Some(targets.into_iter().collect())
        }
    }

    /// Rewrites the request's case-id subjects into per-symbol subjects.
    /// Unresolvable subjects stay verbatim (they surface as unmapped
    /// downstream — never a silent drop). Returns the resolution summary for
    /// the response envelope.
    fn resolve_request(&self, request: &mut astrolabe_anchors::OutcomeAnchorRequest) -> Value {
        let originals = std::mem::take(&mut request.subjects);
        let cases_presented = originals.len();
        let mut cases_resolved = 0usize;
        let mut unresolved_case_ids = Vec::new();
        let mut symbols: BTreeSet<&str> = BTreeSet::new();
        let mut subjects = Vec::new();
        for subject in originals {
            let Some(targets) = self.targets_for(&subject.subject_id) else {
                unresolved_case_ids.push(subject.subject_id.clone());
                subjects.push(subject);
                continue;
            };
            cases_resolved += 1;
            for qualified_name in targets {
                symbols.insert(qualified_name);
                subjects.push(astrolabe_anchors::OutcomeSubject {
                    subject_id: qualified_name.to_string(),
                    anchor_kind: subject.anchor_kind.clone(),
                    value: subject.value.clone(),
                });
            }
        }
        request.subjects = subjects;
        json!({
            "cases_presented": cases_presented,
            "cases_resolved": cases_resolved,
            "unresolved_case_ids": unresolved_case_ids,
            "symbols": symbols.into_iter().collect::<Vec<_>>(),
            // Resolution-graph observability: enough to tell "node map empty",
            // "file paths shaped differently", and "no edges" apart from here.
            "node_map_nodes": self.node_meta.len(),
            "node_map_files": self.file_members.len(),
            "edge_sources": self.out_edges.len(),
            "sample_files": self
                .file_members
                .keys()
                .filter(|file| file.contains("__tests__") || file.contains("monthly"))
                .take(5)
                .collect::<Vec<_>>(),
        })
    }
}

/// Builds a fail-closed refusal envelope with stable `{code, message,
/// remediation}` and HONEST labels.
fn anchor_outcome_refused(
    project: &str,
    source: &str,
    code: &str,
    message: impl Into<String>,
    remediation: &str,
) -> Value {
    json!({
        "schema": ANCHOR_OUTCOME_SCHEMA,
        "project": project,
        "status": "refused",
        "source": source,
        "code": code,
        "message": message.into(),
        "remediation": remediation,
        "anchors_written": 0,
        "trust": "provisional",
        "freshness": "fresh",
        "provenance": ["refusal:pre-commit:no-anchor-written"],
    })
}

/// MCP/CLI dispatch entry point for `anchor_outcome` (routed identically from the
/// JSON-RPC surface and from `astrolabe cli anchor_outcome` via
/// `migration::handle_tool_raw`).
pub(crate) fn handle_anchor_outcome(args_json: &str) -> Result<String, DynError> {
    let args = serde_json::from_str::<Value>(args_json)?;
    let Some(args_obj) = args.as_object() else {
        return tool_error_result("anchor_outcome arguments must be a JSON object");
    };
    let Some(project) = string_arg(args_obj, "project") else {
        return tool_error_result("anchor_outcome requires project");
    };
    let kind = string_arg(args_obj, "kind").unwrap_or("test_run");
    let Some(source) = string_arg(args_obj, "source") else {
        return tool_error_result(
            "anchor_outcome requires a catalog source prefix (ci:/trace:/review:/git:revert:/git:fix:/agent:/survival:)",
        );
    };
    let Some(format) = string_arg(args_obj, "format") else {
        return tool_error_result(
            "anchor_outcome test_run requires format (junit_xml, cargo_test_json, pytest_verbose, go_test_json, or vitest_json)",
        );
    };
    // Large reports (a full vitest run is hundreds of KB) do not fit an
    // inline MCP string argument; report_path reads the file server-side.
    let report_text: String = match string_arg(args_obj, "report")
        .or_else(|| string_arg(args_obj, "report_text"))
    {
        Some(text) => text.to_string(),
        None => match string_arg(args_obj, "report_path") {
            Some(path) => match fs::read_to_string(path) {
                Ok(text) => text,
                Err(error) => {
                    return tool_error_result(&format!(
                        "anchor_outcome could not read report_path {path:?}: {error}"
                    ));
                }
            },
            None => {
                return tool_error_result(
                    "anchor_outcome test_run requires a non-empty report (inline `report` or a server-readable `report_path`)",
                );
            }
        },
    };
    let confidence = match args_obj.get("confidence") {
        None | Some(Value::Null) => None,
        Some(value) => match value.as_f64() {
            Some(number) => Some(number as f32),
            None => return tool_error_result("anchor_outcome confidence must be a number"),
        },
    };
    let observed_at = match args_obj.get("observed_at") {
        None | Some(Value::Null) => now_epoch_seconds().to_string(),
        Some(Value::String(text)) => text.clone(),
        Some(Value::Number(number)) => number.to_string(),
        Some(_) => {
            return tool_error_result(
                "anchor_outcome observed_at must be a non-negative integer epoch",
            );
        }
    };

    let cache_dir = astrolabe_bridge::cbm_cache_dir()?;
    let value = anchor_outcome_json_at(
        &cache_dir,
        project,
        kind,
        source,
        confidence,
        format,
        &report_text,
        &observed_at,
    )?;
    if value.get("status").and_then(Value::as_str) == Some("refused") {
        tool_json_error_result(value)
    } else {
        tool_json_result(value)
    }
}

fn now_epoch_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use astrolabe_ingest::SubjectResolutionNode;

    fn cx(tag: &str) -> calyx_core::CxId {
        calyx_core::CxId::from_input(tag.as_bytes(), 1, b"test-salt")
    }

    fn node(tag: &str, qualified_name: &str, label: &str, file_path: &str) -> SubjectResolutionNode {
        SubjectResolutionNode {
            cx_id: cx(tag),
            qualified_name: qualified_name.to_string(),
            label: label.to_string(),
            file_path: file_path.to_string(),
        }
    }

    #[test]
    fn resolves_case_to_function_symbols_the_test_file_reaches() {
        let nodes = vec![
            node(
                "testfile",
                "app.src.__tests__.monthly-usage.test",
                "File",
                "src/__tests__/monthly-usage.test.ts",
            ),
            node(
                "lineGrams",
                "app.src.lib.monthly-usage.lineGrams",
                "Function",
                "src/lib/monthly-usage.ts",
            ),
            node(
                "someVar",
                "app.src.lib.other.someVar",
                "Variable",
                "src/lib/other.ts",
            ),
        ];
        let edges = vec![
            (cx("testfile"), cx("lineGrams")),
            // Non-Function/Method target label: filtered out.
            (cx("testfile"), cx("someVar")),
        ];
        let index = SubjectGraphIndex::new(&nodes, &edges);

        let targets = index
            .targets_for("C:/repo/src/__tests__/monthly-usage.test.ts::lineGrams unit 'g'")
            .expect("case must resolve");
        assert_eq!(targets, vec!["app.src.lib.monthly-usage.lineGrams"]);

        // Backslashed report paths resolve identically.
        assert!(
            index
                .targets_for("C:\\repo\\src\\__tests__\\monthly-usage.test.ts::case")
                .is_some()
        );
        // Suffix match must respect the path-segment boundary.
        assert!(
            index
                .targets_for("C:/repo/xsrc/__tests__/monthly-usage.test.ts::case")
                .is_none()
        );
        assert!(index.targets_for("C:/repo/src/unknown.test.ts::case").is_none());
        assert!(index.targets_for("no-separator").is_none());
        // Regression: a project-relative case path equal to the node-map key
        // must resolve — the first cut aborted the whole scan via a misplaced
        // `?` whenever any map key was at least as long as the case path.
        assert!(
            index
                .targets_for("src/__tests__/monthly-usage.test.ts::relative case")
                .is_some()
        );
    }
}
