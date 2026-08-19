from pathlib import Path

path = Path("src/ui/mod.rs")
text = path.read_text(encoding="utf-8")

incremental = '''        self.busy = true;
        self.indexing = true;
        self.watcher_reconcile_required = None;
        self.allow_close = false;
        self.close_confirmation_open = false;
        self.similarity_results = None;
'''
incremental_fixed = '''        self.busy = true;
        self.indexing = true;
        self.allow_close = false;
        self.close_confirmation_open = false;
        self.similarity_results = None;
'''
if text.count(incremental) != 1:
    raise SystemExit(f"incremental reconcile flag block: expected 1, found {text.count(incremental)}")
text = text.replace(incremental, incremental_fixed, 1)

rescan = '''    fn start_rescan(&mut self) {
        if self.busy || self.roots.is_empty() {
            return;
        }
        self.busy = true;
        self.indexing = true;
        self.allow_close = false;
'''
rescan_fixed = '''    fn start_rescan(&mut self) {
        if self.busy || self.roots.is_empty() {
            return;
        }
        self.busy = true;
        self.indexing = true;
        self.watcher_reconcile_required = None;
        self.allow_close = false;
'''
if text.count(rescan) != 1:
    raise SystemExit(f"full rescan block: expected 1, found {text.count(rescan)}")
text = text.replace(rescan, rescan_fixed, 1)

path.write_text(text, encoding="utf-8")
print("Watcher reconciliation flag semantics fixed")
