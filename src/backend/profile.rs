use std::collections::HashMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockProfile {
    pub exec_count: u64,
    pub edge_counts: HashMap<usize, u64>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FunctionProfile {
    pub name: String,
    pub blocks: HashMap<usize, BlockProfile>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CompileProfile {
    pub functions: HashMap<String, FunctionProfile>,
}

impl FunctionProfile {
    pub fn block_exec_count(&self, block_id: usize) -> Option<u64> {
        self.blocks.get(&block_id).map(|block| block.exec_count)
    }

    #[allow(dead_code)]
    pub fn edge_count(&self, from: usize, to: usize) -> Option<u64> {
        self.blocks
            .get(&from)
            .and_then(|block| block.edge_counts.get(&to).copied())
    }
}

impl CompileProfile {
    pub const INSTRUMENTATION_MAGIC: [u8; 8] = *b"AZKPGO1\0";

    pub fn parse(text: &str) -> Result<Self, String> {
        let mut functions = HashMap::new();
        let mut current: Option<FunctionProfile> = None;

        for (line_no, raw_line) in text.lines().enumerate() {
            let line = raw_line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parts: Vec<&str> = line.split_whitespace().collect();
            match parts.as_slice() {
                ["function", name] => {
                    if let Some(function) = current.take() {
                        functions.insert(function.name.clone(), function);
                    }
                    current = Some(FunctionProfile {
                        name: (*name).to_string(),
                        blocks: HashMap::new(),
                    });
                }
                ["block", block_id, exec_count] => {
                    let function = current.as_mut().ok_or_else(|| {
                        format!("profile line {}: block outside function", line_no + 1)
                    })?;
                    let block_id = parse_u64(block_id, line_no, "block id")? as usize;
                    let exec_count = parse_u64(exec_count, line_no, "exec count")?;
                    function.blocks.entry(block_id).or_default().exec_count = exec_count;
                }
                ["edge", from, to, count] => {
                    let function = current.as_mut().ok_or_else(|| {
                        format!("profile line {}: edge outside function", line_no + 1)
                    })?;
                    let from = parse_u64(from, line_no, "edge source")? as usize;
                    let to = parse_u64(to, line_no, "edge target")? as usize;
                    let count = parse_u64(count, line_no, "edge count")?;
                    function
                        .blocks
                        .entry(from)
                        .or_default()
                        .edge_counts
                        .insert(to, count);
                    function.blocks.entry(to).or_default();
                }
                ["end"] => {
                    let function = current.take().ok_or_else(|| {
                        format!("profile line {}: end outside function", line_no + 1)
                    })?;
                    functions.insert(function.name.clone(), function);
                }
                _ => {
                    return Err(format!(
                        "profile line {}: unsupported record '{}'",
                        line_no + 1,
                        line
                    ));
                }
            }
        }

        if let Some(function) = current.take() {
            functions.insert(function.name.clone(), function);
        }

        Ok(Self { functions })
    }

    pub fn render(&self) -> String {
        let mut names: Vec<&String> = self.functions.keys().collect();
        names.sort();
        let mut out = String::new();
        for name in names {
            let function = &self.functions[name];
            out.push_str("function ");
            out.push_str(name);
            out.push('\n');

            let mut block_ids: Vec<usize> = function.blocks.keys().copied().collect();
            block_ids.sort_unstable();
            for block_id in block_ids {
                let block = &function.blocks[&block_id];
                out.push_str(&format!("block {} {}\n", block_id, block.exec_count));
                let mut edge_targets: Vec<usize> = block.edge_counts.keys().copied().collect();
                edge_targets.sort_unstable();
                for edge_target in edge_targets {
                    out.push_str(&format!(
                        "edge {} {} {}\n",
                        block_id, edge_target, block.edge_counts[&edge_target]
                    ));
                }
            }
            out.push_str("end\n");
        }
        out
    }

    pub fn merge_instrumentation_raw(
        &mut self,
        function_name: &str,
        raw: &[u8],
    ) -> Result<(), String> {
        let function = self
            .functions
            .get_mut(function_name)
            .ok_or_else(|| format!("profile has no function '{function_name}'"))?;
        let mut block_ids: Vec<usize> = function.blocks.keys().copied().collect();
        block_ids.sort_unstable();
        let expected_len = 16usize
            .checked_add(block_ids.len().saturating_mul(8))
            .ok_or_else(|| String::from("instrumentation profile size overflow"))?;
        if raw.len() != expected_len {
            return Err(format!(
                "raw profile length mismatch: expected {expected_len} bytes for {} blocks, got {}",
                block_ids.len(),
                raw.len()
            ));
        }
        if raw[..8] != Self::INSTRUMENTATION_MAGIC {
            return Err(String::from("raw profile has invalid AZKPGO1 header"));
        }
        let recorded_blocks = u64::from_le_bytes(raw[8..16].try_into().unwrap());
        if recorded_blocks != block_ids.len() as u64 {
            return Err(format!(
                "raw profile block-count mismatch: template has {}, record has {recorded_blocks}",
                block_ids.len()
            ));
        }
        for (position, block_id) in block_ids.iter().copied().enumerate() {
            let start = 16 + position * 8;
            let count = u64::from_le_bytes(raw[start..start + 8].try_into().unwrap());
            function.blocks.get_mut(&block_id).unwrap().exec_count = count;
        }

        // Block counters are exact.  Preserve the template's CFG topology and
        // derive a conservative deterministic edge weight; trace formation
        // falls back to exact successor block counts when edges tie.
        let counts: HashMap<usize, u64> = function
            .blocks
            .iter()
            .map(|(block, profile)| (*block, profile.exec_count))
            .collect();
        for (block_id, block) in &mut function.blocks {
            for (successor, count) in &mut block.edge_counts {
                *count = counts
                    .get(block_id)
                    .copied()
                    .unwrap_or(0)
                    .min(counts.get(successor).copied().unwrap_or(0));
            }
        }
        Ok(())
    }
}

fn parse_u64(value: &str, line_no: usize, field: &str) -> Result<u64, String> {
    value.parse::<u64>().map_err(|_| {
        format!(
            "profile line {}: invalid {} '{}'",
            line_no + 1,
            field,
            value
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_render_profile_round_trip() {
        let profile = CompileProfile::parse(
            r#"
            function runtime_generic
            block 0 10
            edge 0 1 9
            edge 0 2 1
            block 1 9
            block 2 1
            end
            "#,
        )
        .expect("profile should parse");
        assert_eq!(
            profile
                .functions
                .get("runtime_generic")
                .and_then(|f| f.edge_count(0, 1)),
            Some(9)
        );

        let rendered = profile.render();
        let reparsed = CompileProfile::parse(&rendered).expect("rendered profile should parse");
        assert_eq!(profile, reparsed);
    }

    #[test]
    fn raw_block_counters_merge_into_template_with_header_validation() {
        let mut profile = CompileProfile::parse(
            "function runtime_generic\nblock 0 1\nedge 0 1 1\nblock 1 1\nend\n",
        )
        .unwrap();
        let mut raw = Vec::from(CompileProfile::INSTRUMENTATION_MAGIC);
        raw.extend_from_slice(&2u64.to_le_bytes());
        raw.extend_from_slice(&17u64.to_le_bytes());
        raw.extend_from_slice(&9u64.to_le_bytes());
        profile
            .merge_instrumentation_raw("runtime_generic", &raw)
            .unwrap();
        let function = &profile.functions["runtime_generic"];
        assert_eq!(function.block_exec_count(0), Some(17));
        assert_eq!(function.block_exec_count(1), Some(9));
        assert_eq!(function.edge_count(0, 1), Some(9));
    }
}
