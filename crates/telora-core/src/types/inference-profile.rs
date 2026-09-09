// Development-only counters. Default builds contain neither these fields nor
// their increments. Counter-enabled timings must not be used as release baselines.
#[derive(Default)]
struct InferenceProfile {
    depth: std::cell::Cell<u64>,
    roots: std::cell::Cell<u64>,
    nodes: std::cell::Cell<u64>,
    views: std::cell::Cell<u64>,
    body_hits: std::cell::Cell<u64>,
    body_empty: std::cell::Cell<u64>,
    body_stale: std::cell::Cell<u64>,
    body_unindexed: std::cell::Cell<u64>,
    head_queries: std::cell::Cell<u64>,
}

fn profile_increment(counter: &std::cell::Cell<u64>) {
    counter.set(counter.get() + 1);
}

struct NormalizationProfileGuard<'a>(&'a InferenceProfile);

impl InferenceProfile {
    fn normalization(&self) -> NormalizationProfileGuard<'_> {
        if self.depth.get() == 0 {
            profile_increment(&self.roots);
        }
        profile_increment(&self.depth);
        profile_increment(&self.nodes);
        NormalizationProfileGuard(self)
    }
}

impl Drop for NormalizationProfileGuard<'_> {
    fn drop(&mut self) {
        self.0.depth.set(self.0.depth.get() - 1);
    }
}

impl Drop for InferenceProfile {
    fn drop(&mut self) {
        if std::env::var_os("TELORA_INFERENCE_PROFILE").is_none() {
            return;
        }
        eprintln!(
            "{}",
            serde_json::json!({
                "record": "telora.inference-profile",
                "normalization_roots": self.roots.get(),
                "normalization_nodes": self.nodes.get(),
                "descriptor_views": self.views.get(),
                "body_cache_hits": self.body_hits.get(),
                "body_cache_empty": self.body_empty.get(),
                "body_cache_stale": self.body_stale.get(),
                "body_unindexed": self.body_unindexed.get(),
                "head_queries": self.head_queries.get(),
            })
        );
    }
}
