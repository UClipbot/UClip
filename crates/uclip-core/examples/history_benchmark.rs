//! Synthetic stress data bypasses the product's 1000-row capacity only for search measurement.
use rusqlite::{params, Connection};
use std::time::Instant;
use uclip_core::{HistoryQuery, Storage};

fn main() {
    let mut results = vec![];
    for count in [100, 1000, 10_000, 100_000] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stress.sqlite3");
        drop(Storage::open(&path).unwrap());
        let mut conn = Connection::open(&path).unwrap();
        let tx = conn.transaction().unwrap();
        {
            let mut insert=tx.prepare("INSERT INTO clips(id,kind,text,data,width,height,fingerprint,captured_at,sequence,revision) VALUES(?1,?2,?3,?4,?5,?6,?7,0,?8,0)").unwrap();
            for i in 0..count {
                let image = i % 1000 == 999;
                let text = if image {
                    String::new()
                } else {
                    format!(
                        "fixture {i:06} 中文\nSECOND line {}",
                        if i % 100 == 0 {
                            "x".repeat(4096)
                        } else {
                            "A".repeat(64)
                        }
                    )
                };
                let data = if image {
                    vec![128u8; 256 * 256 * 4]
                } else {
                    vec![]
                };
                insert
                    .execute(params![
                        uuid::Uuid::from_u128(i as u128 + 1).to_string(),
                        if image { "image" } else { "text" },
                        text,
                        data,
                        if image { Some(256) } else { None },
                        if image { Some(256) } else { None },
                        vec![0u8; 32],
                        i
                    ])
                    .unwrap();
            }
        }
        tx.commit().unwrap();
        let payload: i64 = conn
            .query_row(
                "SELECT sum(length(CAST(text AS BLOB))+length(data)) FROM clips",
                [],
                |r| r.get(0),
            )
            .unwrap();
        drop(conn);
        let storage = Storage::open(&path).unwrap();
        for (query, regex, expected) in [
            ("中文", false, count - count / 1000),
            ("(?m)^SECOND line", true, count - count / 1000),
            ("missing fixture", false, 0),
        ] {
            let mut times = vec![];
            for _ in 0..21 {
                let start = Instant::now();
                let page = storage
                    .history(HistoryQuery {
                        query: query.into(),
                        regex,
                        kind: None,
                        offset: 0,
                        expected_revision: None,
                    })
                    .unwrap();
                times.push(start.elapsed().as_secs_f64() * 1000.0);
                assert_eq!(page.total, expected);
                assert_eq!(page.items.len(), usize::min(25, expected as usize));
            }
            let cold = times.remove(0);
            times.sort_by(f64::total_cmp);
            results.push(serde_json::json!({"rows":count,"payloadBytes":payload,"query":query,"regex":regex,"firstMs":cold,"warmP50Ms":times[9],"warmP95Ms":times[18],"warmMaxMs":times[19],"databaseBytes":std::fs::metadata(&path).unwrap().len()}));
        }
    }
    println!("{}", serde_json::to_string_pretty(&results).unwrap());
}
