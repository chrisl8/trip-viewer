use rusqlite::{params, Connection};

fn main() {
    let path = "/run/media/chris10/Matrix/Wolfbox Dashcam/.tripviewer/tripviewer.db";
    {
        let conn = Connection::open(path).unwrap();
        conn.execute("CREATE TABLE IF NOT EXISTS persistence_test (k TEXT PRIMARY KEY, v TEXT)", []).unwrap();
        let n = conn.execute("INSERT OR REPLACE INTO persistence_test (k, v) VALUES (?1, ?2)", params!["probe", "wrote_at_now"]).unwrap();
        println!("INSERT returned rows_affected={n}");
    }
    {
        let conn = Connection::open(path).unwrap();
        let v: Option<String> = conn.query_row(
            "SELECT v FROM persistence_test WHERE k = ?1",
            params!["probe"],
            |r| r.get(0),
        ).ok();
        println!("read: {v:?}");
    }
}
