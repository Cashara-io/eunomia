//! Spike S1 (foundation-plan 3.3): database per org at scale, on the in-memory engine.
//!
//!   S1_N=200 cargo test --test spike_s1 -- --ignored --nocapture
//!
//! Provisions N org databases, each migrated (HNSW 1536 + two FULLTEXT indexes) and holding 100
//! records with embeddings, then reports: provisioning time per org, resident memory, first-query
//! latency through a cold `pool.for_org` (and a vector search through it), and how long a one-field
//! migration takes to fan out across every org. It is a script, not a gate: nothing here fails on a
//! slow number. Numbers from a debug build are pessimistic; the plan's targets (5,000 to 10,000
//! orgs, recall p95 under 150 ms cold, fan-out under 30 minutes) are on a real server.
mod common;

use std::time::{Duration, Instant};

use eunomia_backend::pool::OrgId;
use rand::{Rng, SeedableRng};
use rand::rngs::StdRng;

fn rss_mb() -> f64 {
    let out = std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().expect("ps");
    String::from_utf8_lossy(&out.stdout).trim().parse::<f64>().unwrap_or(0.0) / 1024.0
}

fn pct(sorted: &[Duration], p: f64) -> Duration {
    sorted[((sorted.len() as f64 - 1.0) * p).round() as usize]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn summary(label: &str, mut v: Vec<Duration>) -> String {
    v.sort();
    format!("{label}: p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms", ms(pct(&v, 0.5)), ms(pct(&v, 0.95)), ms(*v.last().unwrap()))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "spike: S1_N=200 cargo test --test spike_s1 -- --ignored --nocapture"]
async fn s1_database_per_org() {
    let n: usize = std::env::var("S1_N").ok().and_then(|v| v.parse().ok()).unwrap_or(200);
    // keep every handle warm during the load; the cold pass evicts them one by one
    // SAFETY: set before any thread reads it (the pool reads it once in AppState::build)
    unsafe { std::env::set_var("EUNOMIA_ORG_POOL_CAP", (n + 10).to_string()) };
    let state = common::bare_state().await;
    let p = state.provisioner.as_ref().expect("provisioning feature").clone();
    let rss0 = rss_mb();
    let mut rng = StdRng::seed_from_u64(1);

    // -- provision + load ------------------------------------------------------------------
    let mut orgs = Vec::with_capacity(n);
    let (mut prov, mut load) = (Vec::new(), Vec::new());
    let started = Instant::now();
    for i in 0..n {
        let org = OrgId::new();
        let t = Instant::now();
        p.provision_org(&state.control, org, &format!("s1-{i}")).await.expect("provision");
        prov.push(t.elapsed());

        let db = state.pool.for_org(&org).await.expect("for_org");
        let t = Instant::now();
        let rows: Vec<serde_json::Value> = (0..100)
            .map(|r| {
                let emb: Vec<f32> = (0..1536).map(|_| rng.r#gen::<f32>() - 0.5).collect();
                serde_json::json!({
                    "id": format!("cache_record:r{r}"), "owner": "user:s1", "source": "s1", "type": "note", "external_id": format!("{r}"),
                    "title": format!("Quarterly note {r} for org {i}"), "body_text": format!("renewal terms and budget review item {r} org {i}"),
                    "content_hash": format!("h{r}"), "ingested_at": "2026-01-01T00:00:00Z", "updated_at": "2026-01-01T00:00:00Z", "embedding": emb,
                })
            })
            .collect();
        let sql = "INSERT INTO cache_record (SELECT *, <datetime>ingested_at AS ingested_at, <datetime>updated_at AS updated_at, $owner AS owner FROM $rows)";
        db.test_raw().query(sql).bind(("owner", eunomia_backend::rid::parse("user:s1").unwrap())).bind(("rows", rows)).await.expect("insert").check().expect("insert ok");
        load.push(t.elapsed());
        orgs.push(org);
        if (i + 1) % 50 == 0 {
            eprintln!("  {} orgs in {:.0}s, rss {:.0} MB", i + 1, started.elapsed().as_secs_f64(), rss_mb());
        }
    }
    let rss1 = rss_mb();
    println!("\n=== S1 (N={n}, in-memory engine, debug build unless --release) ===");
    println!("{}", summary("provision one org (database + 9 migrations incl. HNSW + 2 FULLTEXT + user + routing row)", prov));
    println!("{}", summary("load 100 records x 1536 dims into one org", load));
    println!("memory: {:.0} MB before, {:.0} MB after {n} orgs ({:.2} MB per org)", rss0, rss1, (rss1 - rss0) / n as f64);

    // -- cold for_org + first query + vector search ------------------------------------------
    let (mut cold, mut first_query, mut knn, mut warm) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let q: Vec<f32> = (0..1536).map(|_| rng.r#gen::<f32>() - 0.5).collect();
    let owner = eunomia_backend::rid::parse("user:s1").unwrap();
    for org in &orgs {
        state.pool.evict(org);
        let t = Instant::now();
        let db = state.pool.for_org(org).await.expect("cold for_org");
        cold.push(t.elapsed());
        let t = Instant::now();
        let mut res = db.test_raw().query("SELECT count() FROM cache_record GROUP ALL").await.unwrap();
        let _: Vec<serde_json::Value> = res.take(0).unwrap();
        first_query.push(t.elapsed());
        let t = Instant::now();
        let ids = eunomia_backend::cache::search::nearest_ids(&db, &eunomia_backend::cache::search::RecordScope::own(&owner), q.clone(), &Default::default(), 10).await.unwrap();
        knn.push(t.elapsed());
        assert!(!ids.is_empty());
        let t = Instant::now();
        let again = state.pool.for_org(org).await.unwrap();
        let _ = again.org();
        warm.push(t.elapsed());
    }
    println!("{}", summary("cold pool.for_org (sign in as the org's database user)", cold));
    println!("{}", summary("first query after a cold for_org (count)", first_query));
    println!("{}", summary("vector search (nearest_ids, k=10) on the cold handle", knn));
    println!("{}", summary("warm pool.for_org (cache hit)", warm));
    println!("pool: {} handles open (cap {})", state.pool.open_handles(), n + 10);

    // -- one-field migration fan-out ---------------------------------------------------------
    let t = Instant::now();
    let mut per = Vec::new();
    for org in &orgs {
        let s = p.session_on(&org.db_name()).await.unwrap();
        let t1 = Instant::now();
        s.query("DEFINE FIELD OVERWRITE s1_flag ON cache_record TYPE option<string>").await.unwrap().check().unwrap();
        per.push(t1.elapsed());
    }
    let total = t.elapsed();
    println!("{}", summary("one-field migration on one org", per));
    println!(
        "fan-out of a one-field migration over {n} orgs, one at a time: {:.1} s ({:.1} ms/org); at 5,000 orgs that is about {:.1} min",
        total.as_secs_f64(),
        ms(total) / n as f64,
        total.as_secs_f64() * (5000.0 / n as f64) / 60.0
    );
    println!("total run {:.0} s, final rss {:.0} MB", started.elapsed().as_secs_f64(), rss_mb());
}
