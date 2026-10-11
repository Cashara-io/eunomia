//! The "vector cloud": every memory and synced record (see `cache::search::RecordScope`) in one or
//! more vaults, projected from its embedding space
//! down to 3D with PCA so the UI can draw the overall shape of what's stored
//! -- one coloured cloud per vault, layered in the same axes.
//!
//! All layers share ONE projection (fit on the union of points), so two
//! vaults, or a vault and its merge, are directly comparable.
//!
//! Space: real embeddings when the server has them (memories are embedded on
//! demand through the `embed_cache` memo; records use their stored vector),
//! otherwise a local hashed bag-of-words vector -- no key, no network, still
//! puts texts that share words near each other.

use surrealdb::types::SurrealValue;
use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use surrealdb::types::RecordId;
use crate::rid::RecordIdExt;

use crate::cache::search as cs;
use crate::config::Settings;
use crate::pool::OrgDb;
use crate::store;
use crate::embeddings::service as embeddings;
use crate::error::AppResult;
use crate::vaults::service as vaults_service;

/// Points per vault layer -- keeps the payload and the PCA cheap.
const MAX_PER_LAYER: usize = 1500;
const LEXICAL_DIM: usize = 512;

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct CloudPoint {
    pub id: String,
    pub vault: String,
    /// "record", or the memory's type (world/experience/observation)
    pub kind: String,
    pub label: String,
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct Cloud {
    /// "semantic" (model embeddings) or "lexical" (hashed words, no model)
    pub space: &'static str,
    pub points: Vec<CloudPoint>,
}

struct Item {
    id: String,
    vault: String,
    kind: String,
    text: String,
    stored: Option<Vec<f32>>,
}

fn fnv1a(s: &str) -> u64 {
    s.bytes().fold(0xcbf29ce484222325u64, |h, b| (h ^ b as u64).wrapping_mul(0x100000001b3))
}

/// Hashed bag-of-words (signed feature hashing), L2-normalised.
pub fn lexical_vector(text: &str) -> Vec<f32> {
    let mut v = vec![0f32; LEXICAL_DIM];
    for token in text.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|t| t.len() > 1) {
        let h = fnv1a(token);
        let sign = if h >> 63 == 0 { 1.0 } else { -1.0 };
        v[(h % LEXICAL_DIM as u64) as usize] += sign;
    }
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
}

/// Top-3 principal components by power iteration (no linear-algebra crate
/// needed at these sizes), returning each row's coordinates scaled so the
/// largest |coordinate| is 1. Rows must share one length.
pub fn pca3(rows: &[Vec<f32>]) -> Vec<[f32; 3]> {
    let n = rows.len();
    if n == 0 {
        return Vec::new();
    }
    let d = rows[0].len();
    let mut mean = vec![0f32; d];
    for r in rows {
        for (m, x) in mean.iter_mut().zip(r) {
            *m += x / n as f32;
        }
    }
    let centered: Vec<Vec<f32>> = rows.iter().map(|r| r.iter().zip(&mean).map(|(x, m)| x - m).collect()).collect();
    let dot = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();

    let mut comps: Vec<Vec<f32>> = Vec::new();
    for k in 0..3 {
        // deterministic, non-degenerate start
        let mut v: Vec<f32> = (0..d).map(|i| ((i * 7 + k * 13) % 11) as f32 - 5.0 + 0.5).collect();
        // project earlier components out of v before AND after multiplying: with a
        // dominant first axis, float error alone would otherwise pull it back in
        let deflate = |w: &mut Vec<f32>, comps: &[Vec<f32>]| {
            for c in comps {
                let c_dot = dot(w, c);
                w.iter_mut().zip(c).for_each(|(wi, ci)| *wi -= c_dot * ci);
            }
        };
        for _ in 0..60 {
            deflate(&mut v, &comps);
            let proj: Vec<f32> = centered.iter().map(|r| dot(r, &v)).collect();
            let mut w = vec![0f32; d];
            for (r, p) in centered.iter().zip(&proj) {
                for (wi, x) in w.iter_mut().zip(r) {
                    *wi += x * p;
                }
            }
            deflate(&mut w, &comps);
            let norm = dot(&w, &w).sqrt();
            if norm < 1e-6 {
                v = vec![0f32; d]; // no variance left in this direction: flat axis
                break;
            }
            v = w.into_iter().map(|x| x / norm).collect();
        }
        comps.push(v);
    }

    let mut coords: Vec<[f32; 3]> =
        centered.iter().map(|r| [dot(r, &comps[0]), dot(r, &comps[1]), dot(r, &comps[2])]).collect();
    let max = coords.iter().flat_map(|c| c.iter()).fold(0f32, |m, x| m.max(x.abs()));
    if max > 0.0 {
        coords.iter_mut().for_each(|c| c.iter_mut().for_each(|x| *x /= max));
    }
    coords
}

fn label(text: &str) -> String {
    let t: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.chars().count() > 90 {
        t.chars().take(90).collect::<String>() + "\u{2026}"
    } else {
        t
    }
}

async fn layer_items(db: &OrgDb, scope: &cs::RecordScope, vault: &RecordId) -> AppResult<Vec<Item>> {
    #[derive(Deserialize, SurrealValue)]
    struct Mem {
        id: RecordId,
        #[serde(default)]
        #[surreal(default)]
        text: String,
        #[serde(rename = "type", default)]
        #[surreal(rename = "type", default)]
        mem_type: String,
    }
    let mut res = store::cache::MEMORY_FOR_EMBED
        .on(db)
        .bind(("vault", vault.clone()))
        .bind(("limit", MAX_PER_LAYER as i64))
        .await?;
    let mems: Vec<Mem> = res.take(0)?;
    let mut items: Vec<Item> = mems
        .into_iter()
        .filter(|m| !m.text.trim().is_empty())
        .map(|m| Item { id: m.id.to_string(), vault: vault.to_string(), kind: m.mem_type, text: m.text, stored: None })
        .collect();

    if items.len() < MAX_PER_LAYER {
        #[derive(Deserialize, SurrealValue)]
        struct Rec {
            id: RecordId,
            #[serde(default)]
            #[surreal(default)]
            title: String,
            #[serde(default)]
            #[surreal(default)]
            body_text: String,
            #[serde(default)]
            #[surreal(default)]
            embedding: Option<Vec<f32>>,
        }
        let mut res = scope.bind(store::cache::RECORDS_FOR_EMBED.on(db).bind(("limit", (MAX_PER_LAYER - items.len()) as i64))).await?;
        let recs: Vec<Rec> = res.take(0)?;
        items.extend(recs.into_iter().map(|r| Item {
            id: format!("cache_record:{}", cs::caller_id(&r.id, scope.owner())),
            vault: vault.to_string(),
            kind: "record".to_string(),
            text: format!("{}\n{}", r.title, r.body_text).trim().to_string(),
            stored: r.embedding.filter(|e| e.len() == embeddings::DIM),
        }));
    }
    Ok(items)
}

/// The cloud for `vault_ids` (default: the caller's personal vault). Every
/// vault must be one the caller belongs to.
pub async fn cloud(db: &OrgDb, settings: &Settings, owner: &RecordId, vault_ids: &[RecordId]) -> AppResult<Cloud> {
    let personal = vaults_service::personal_vault_id(db, owner).await?;
    let vaults: Vec<RecordId> =
        if vault_ids.is_empty() { vec![vaults_service::default_vault_id(db, owner).await?] } else { vault_ids.to_vec() };

    let mut items = Vec::new();
    #[allow(clippy::mutable_key_type)] // RecordId hashes by value; the interior mutability is never touched
    let mut seen = HashSet::new();
    for v in &vaults {
        if !seen.insert(v.clone()) {
            continue;
        }
        crate::authz::authorize(db, owner, crate::authz::Action::ReadMemories, v).await?;
        items.extend(layer_items(db, &cs::RecordScope::vaults(owner, &personal, std::slice::from_ref(v)), v).await?);
    }

    let semantic = if embeddings::available(db, settings, owner).await {
        let texts: Vec<String> = items.iter().filter(|i| i.stored.is_none()).map(|i| i.text.clone()).collect();
        match embeddings::embed(db, settings, &texts, Some(owner)).await {
            Ok(vecs) => {
                let mut fresh = vecs.into_iter();
                Some(items.iter().map(|i| i.stored.clone().or_else(|| fresh.next()).unwrap_or_default()).collect::<Vec<_>>())
            }
            Err(e) => {
                tracing::warn!("cloud: embeddings failed, using lexical space: {}", e.message);
                None
            }
        }
    } else {
        None
    };
    let (space, vectors) = match semantic {
        Some(v) if v.iter().all(|x| x.len() == embeddings::DIM) => ("semantic", v),
        _ => ("lexical", items.iter().map(|i| lexical_vector(&i.text)).collect()),
    };

    let coords = pca3(&vectors);
    let points = items
        .into_iter()
        .zip(coords)
        .map(|(i, [x, y, z])| CloudPoint { id: i.id, vault: i.vault, kind: i.kind, label: label(&i.text), x, y, z })
        .collect();
    Ok(Cloud { space, points })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cos(a: &[f32], b: &[f32]) -> f32 {
        a.iter().zip(b).map(|(x, y)| x * y).sum()
    }

    #[test]
    fn lexical_vectors_are_unit_length_and_word_sensitive() {
        let a = lexical_vector("Ada wrote the first algorithm");
        let b = lexical_vector("the first algorithm, written by Ada");
        let c = lexical_vector("quarterly invoice for cloud hosting");
        assert!((cos(&a, &a) - 1.0).abs() < 1e-5);
        assert!(cos(&a, &b) > cos(&a, &c));
        assert_eq!(lexical_vector("").iter().filter(|x| **x != 0.0).count(), 0);
    }

    #[test]
    fn pca_recovers_the_main_axis_and_scales_to_unit() {
        // points spread along one direction in 5D, tiny noise elsewhere
        let rows: Vec<Vec<f32>> =
            (0..20).map(|i| { let t = i as f32 - 10.0; vec![t, 2.0 * t, 0.01 * (i % 3) as f32, -t, 0.0] }).collect();
        let c = pca3(&rows);
        assert_eq!(c.len(), 20);
        let max = c.iter().flat_map(|p| p.iter()).fold(0f32, |m, x| m.max(x.abs()));
        assert!((max - 1.0).abs() < 1e-5);
        // all the spread lands on the first component
        let spread = |k: usize| c.iter().map(|p| p[k].abs()).fold(0f32, f32::max);
        assert!(spread(0) > 0.99);
        assert!(spread(1) < 0.05 && spread(2) < 0.05);
    }

    #[test]
    fn pca_handles_empty_and_single_inputs() {
        assert!(pca3(&[]).is_empty());
        assert_eq!(pca3(&[vec![1.0, 2.0]]), vec![[0.0, 0.0, 0.0]]);
    }

    #[test]
    fn labels_are_trimmed() {
        assert_eq!(label("  a\n b  "), "a b");
        assert!(label(&"x ".repeat(100)).ends_with('\u{2026}'));
    }
}
