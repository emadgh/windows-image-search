use crate::{db::CollectionMembership, portable, thumbnail_cache};
use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Default)]
pub(super) struct StatisticsState {
    pub folder: Option<PathBuf>,
    key: Option<(i64, Option<PathBuf>)>,
    pending: Option<mpsc::Receiver<Result<Statistics, String>>>,
    pub result: Option<Result<Statistics, String>>,
    pub updated: Option<Instant>,
}

impl StatisticsState {
    pub fn reset(&mut self) {
        self.key = None;
        self.result = None;
        self.updated = None;
        // Keep one worker at a time; its result is discarded if selection changed.
    }

    pub fn poll(
        &mut self,
        id: i64,
        db: &Path,
        membership: &CollectionMembership,
        ctx: &eframe::egui::Context,
        refresh: bool,
    ) {
        let key = (id, self.folder.clone());
        if let Some(rx) = &self.pending {
            match rx.try_recv() {
                Ok(result) => {
                    self.pending = None;
                    if self.key.as_ref() == Some(&key) {
                        self.result = Some(result);
                        self.updated = Some(Instant::now());
                    } else {
                        self.key = None;
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.pending = None;
                    self.key = None;
                    self.result = Some(Err("Statistics worker stopped".into()));
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self.key.as_ref() != Some(&key) {
            self.result = None;
            self.updated = None;
        }
        if self.pending.is_none() && (self.key.as_ref() != Some(&key) || refresh) {
            self.key = Some(key);
            let db = db.to_owned();
            let membership = membership.clone();
            let folder = self.folder.clone();
            let ctx = ctx.clone();
            let (tx, rx) = mpsc::channel();
            self.pending = Some(rx);
            std::thread::spawn(move || {
                let result =
                    collect(&db, &membership, folder.as_deref()).map_err(|e| format!("{e:#}"));
                let _ = tx.send(result);
                ctx.request_repaint();
            });
        }
        if self.pending.is_some() {
            ctx.request_repaint_after(Duration::from_millis(200));
        }
    }
    pub fn loading(&self) -> bool {
        self.pending.is_some()
    }
}

#[derive(Default, Debug)]
pub(super) struct Statistics {
    pub discovered: usize,
    pub indexed: usize,
    pub clip: usize,
    pub hashes: usize,
    pub colors: usize,
    pub material: usize,
    pub thumbnails: usize,
    pub thumbnail_bytes: u64,
    pub unavailable: usize,
    pub source_bytes: u64,
    pub errors: usize,
    pub error_reasons: BTreeMap<String, usize>,
    pub formats: BTreeMap<String, usize>,
    pub face_images: usize,
    pub faces: usize,
    pub face_vectors: usize,
    pub no_faces: usize,
    pub warnings: Vec<String>,
}

fn open_read_only(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    conn.busy_timeout(Duration::from_secs(2))?;
    conn.execute_batch("BEGIN DEFERRED")?;
    Ok(conn)
}
fn table_exists(conn: &Connection, name: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1)",
        [name],
        |r| r.get(0),
    )?)
}
fn in_scope(path: &Path, membership: &CollectionMembership, folder: Option<&Path>) -> bool {
    if let Some(folder) = folder {
        return path.starts_with(folder);
    }
    membership.folders.iter().any(|f| path.starts_with(f))
        || membership.files.iter().any(|f| f == path)
}

pub(super) fn collect(
    db: &Path,
    membership: &CollectionMembership,
    folder: Option<&Path>,
) -> Result<Statistics> {
    let conn = open_read_only(db)?;
    let mut out = Statistics::default();
    let mut selected = HashSet::new();
    let mut roots: HashMap<PathBuf, HashSet<PathBuf>> = HashMap::new();
    let mut sources = Vec::new();
    let mut stmt = conn.prepare("SELECT path,root,size,extension,embedding IS NOT NULL AND embedding_dim>0 AND length(embedding)=embedding_dim*4, visual_hash IS NOT NULL, color_histogram IS NOT NULL, material_texture IS NOT NULL AND material_texture_version=?1 FROM images")?;
    let rows = stmt.query_map([crate::material_texture::VERSION], |r| {
        Ok((
            PathBuf::from(r.get::<_, String>(0)?),
            PathBuf::from(r.get::<_, String>(1)?),
            r.get::<_, i64>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<bool>>(4)?.unwrap_or(false),
            r.get::<_, bool>(5)?,
            r.get::<_, bool>(6)?,
            r.get::<_, bool>(7)?,
        ))
    })?;
    for row in rows {
        let (path, root, size, ext, clip, hash, color, material) = row?;
        if !in_scope(&path, membership, folder) {
            continue;
        }
        selected.insert(path.clone());
        roots.entry(root.clone()).or_default().insert(path.clone());
        out.indexed += 1;
        out.source_bytes += size.max(0) as u64;
        out.clip += usize::from(clip);
        out.hashes += usize::from(hash);
        out.colors += usize::from(color);
        out.material += usize::from(material);
        *out.formats.entry(ext.to_ascii_lowercase()).or_default() += 1;
        sources.push((path, root));
    }
    let mut discovered = selected.clone();
    for row in conn
        .prepare("SELECT path FROM discovered_images")?
        .query_map([], |r| r.get::<_, String>(0))?
    {
        let path = PathBuf::from(row?);
        if in_scope(&path, membership, folder) {
            discovered.insert(path);
        }
    }
    out.discovered = discovered.len();
    if table_exists(&conn, "decode_failures")? {
        for row in conn
            .prepare("SELECT path,reason FROM decode_failures")?
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
        {
            let (path, reason) = row?;
            if in_scope(Path::new(&path), membership, folder) {
                out.errors += 1;
                *out.error_reasons.entry(reason).or_default() += 1;
            }
        }
    }
    drop(stmt);
    drop(conn); // Do not pin the session WAL during filesystem work.
    for (path, root) in sources {
        if !path.is_file() {
            out.unavailable += 1;
            continue;
        }
        // Presence of the current source-keyed cache file, not a decode/quality claim.
        let portable = thumbnail_cache::cache_path_for_root(&root, &path).ok();
        let cached = portable.filter(|p| p.is_file()).unwrap_or_else(|| {
            thumbnail_cache::cache_path(&thumbnail_cache::cache_dir_for_db(db), &path)
        });
        if let Ok(meta) = std::fs::metadata(cached) {
            if meta.is_file() && meta.len() > 0 {
                out.thumbnails += 1;
                out.thumbnail_bytes += meta.len();
            }
        }
    }
    for (root, root_selected) in roots {
        let result = (|| -> Result<(usize, usize, usize, usize)> {
            let c = open_read_only(&portable::index_db_path(&root))?;
            if !table_exists(&c, "face_detection_state")? {
                return Ok((0, 0, 0, 0));
            }
            let mut images = 0;
            let mut empty = 0;
            let mut faces = 0;
            let mut vectors = 0;
            for row in c
                .prepare("SELECT image_path,face_count FROM face_detection_state")?
                .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            {
                let (path, count) = row?;
                let absolute = portable::absolute_source_path(&root, Path::new(&path))?;
                if root_selected.contains(&absolute) {
                    images += 1;
                    empty += usize::from(count == 0);
                }
            }
            if table_exists(&c, "faces")? {
                let has_vectors = table_exists(&c, "face_embeddings")?;
                let sql = if has_vectors {
                    "SELECT f.image_path, EXISTS(SELECT 1 FROM face_embeddings e WHERE e.face_id=f.face_id AND e.dimension>0 AND length(e.embedding)=e.dimension*4) FROM faces f"
                } else {
                    "SELECT image_path,0 FROM faces"
                };
                for row in c
                    .prepare(sql)?
                    .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))?
                {
                    let (path, vector) = row?;
                    let absolute = portable::absolute_source_path(&root, Path::new(&path))?;
                    if root_selected.contains(&absolute) {
                        faces += 1;
                        vectors += usize::from(vector);
                    }
                }
            }
            Ok((images, empty, faces, vectors))
        })();
        match result {
            Ok((i, e, f, v)) => {
                out.face_images += i;
                out.no_faces += e;
                out.faces += f;
                out.face_vectors += v;
            }
            Err(e) => out.warnings.push(format!(
                "Face statistics unavailable for {}: {e}",
                root.display()
            )),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            let nonce = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let p =
                std::env::temp_dir().join(format!("wis-statistics-{}-{nonce}", std::process::id()));
            std::fs::create_dir_all(&p).unwrap();
            Self(p)
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn statistics_are_scoped_deduplicated_and_read_only() {
        let f = Fixture::new();
        let root = f.0.join("photos");
        std::fs::create_dir_all(portable::index_dir(&root)).unwrap();
        let path = root.join("one.jpg");
        std::fs::write(&path, b"source").unwrap();
        let db = f.0.join("session.sqlite3");
        let c = Connection::open(&db).unwrap();
        c.execute_batch("CREATE TABLE images(path TEXT,root TEXT,size INTEGER,extension TEXT,embedding BLOB,embedding_dim INTEGER,visual_hash INTEGER,color_histogram BLOB,material_texture BLOB,material_texture_version INTEGER); CREATE TABLE discovered_images(path TEXT); CREATE TABLE decode_failures(path TEXT,reason TEXT);").unwrap();
        c.execute(
            "INSERT INTO images VALUES(?1,?2,6,'JPG',zeroblob(8),2,1,zeroblob(4),zeroblob(4),?3)",
            params![
                path.to_string_lossy(),
                root.to_string_lossy(),
                crate::material_texture::VERSION
            ],
        )
        .unwrap();
        c.execute(
            "INSERT INTO discovered_images VALUES(?1),(?2)",
            params![
                path.to_string_lossy(),
                root.join("failed.jpg").to_string_lossy()
            ],
        )
        .unwrap();
        c.execute(
            "INSERT INTO decode_failures VALUES(?1,'Invalid image')",
            params![root.join("failed.jpg").to_string_lossy()],
        )
        .unwrap();
        // Prefix-like sibling must not enter the selected folder's counts.
        c.execute("INSERT INTO images SELECT ?1,root,size,extension,NULL,NULL,visual_hash,color_histogram,material_texture,material_texture_version FROM images LIMIT 1",params![f.0.join("photos-other/outside.jpg").to_string_lossy()]).unwrap();
        drop(c);
        let face_db = portable::index_db_path(&root);
        let c = Connection::open(&face_db).unwrap();
        c.execute_batch("CREATE TABLE face_detection_state(image_path TEXT,face_count INTEGER); INSERT INTO face_detection_state VALUES('one.jpg',2); CREATE TABLE faces(face_id TEXT,image_path TEXT); INSERT INTO faces VALUES('f1','one.jpg'),('f2','one.jpg'); CREATE TABLE face_embeddings(face_id TEXT,dimension INTEGER,embedding BLOB); INSERT INTO face_embeddings VALUES('f1',2,zeroblob(8)),('f2',2,zeroblob(4));").unwrap();
        drop(c);
        let thumb = thumbnail_cache::cache_path_for_root(&root, &path).unwrap();
        std::fs::create_dir_all(thumb.parent().unwrap()).unwrap();
        std::fs::write(&thumb, b"not a jpeg; must not be deleted").unwrap();
        let before = std::fs::read(&db).unwrap();
        let face_before = std::fs::read(&face_db).unwrap();
        let membership = CollectionMembership {
            folders: vec![root.clone(), root.clone()],
            files: vec![path],
        };
        let stats = collect(&db, &membership, None).unwrap();
        assert_eq!(
            (
                stats.indexed,
                stats.discovered,
                stats.clip,
                stats.thumbnails,
                stats.errors
            ),
            (1, 2, 1, 1, 1)
        );
        assert_eq!(
            (stats.face_images, stats.faces, stats.face_vectors),
            (1, 2, 1)
        );
        assert_eq!(stats.source_bytes, 6);
        assert!(stats.warnings.is_empty());
        assert_eq!(collect(&db, &membership, Some(&root)).unwrap().indexed, 1);
        assert_eq!(std::fs::read(&db).unwrap(), before);
        assert_eq!(std::fs::read(&face_db).unwrap(), face_before);
        assert_eq!(
            std::fs::read(&thumb).unwrap(),
            b"not a jpeg; must not be deleted"
        );
        let readonly = open_read_only(&db).unwrap();
        assert!(readonly.execute("DELETE FROM images", []).is_err());
        drop(readonly);
        std::fs::remove_file(&face_db).unwrap();
        let partial = collect(&db, &membership, None).unwrap();
        assert_eq!(partial.warnings.len(), 1);
        assert!(!face_db.exists());
    }

    #[test]
    fn statistics_do_not_create_missing_database() {
        let f = Fixture::new();
        let missing = f.0.join("missing.sqlite3");
        assert!(collect(&missing, &CollectionMembership::default(), None).is_err());
        assert!(!missing.exists());
    }
}
