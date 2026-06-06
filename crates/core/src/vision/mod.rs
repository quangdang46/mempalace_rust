//! Vision module - image embedding, search, and management.
//! Ported from mempalace's vision-search.ts, image-refs.ts, image-store.ts, image-quota-cleanup.ts

pub mod embedding_provider;
pub mod image_quota;
pub mod image_refs;
pub mod image_store;
pub mod vision_search;

pub use embedding_provider::{
    cosine_similarity, DimensionGuard, EmbeddingProvider, StoredEmbedding, StubEmbeddingProvider,
};
pub use image_quota::ImageQuotaCleanup;
pub use image_refs::ImageRefStore;
pub use image_store::{
    delete_image, images_dir, is_managed_image_path, max_bytes, save_image_to_disk, touch_image,
};
pub use vision_search::{EmbedResult, SearchResult, VisionSearchStore};
