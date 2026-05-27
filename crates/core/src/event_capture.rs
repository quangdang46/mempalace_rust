// =====================================================================
// EventCapture — observer trait for runtime telemetry (Phase 4 / mr-6ke2)
// =====================================================================
//
// `EventCapture` is a minimal observer interface that lets a hosting
// agent (jcode) forward runtime events into mempalace for cross-agent
// memory enrichment. The trait is intentionally narrow — it only captures
// events that are useful for memory consolidation: session lifecycle,
// user prompts, tool invocations, and memory writes.
//
// Implementors forward events to an internal event bus (Palace) or to
// an external telemetry sink. The trait is object-safe so multiple
// listeners can be registered.
//
// Thread safety: All methods take `&self` — implementors must be
// thread-safe (Send + Sync) since jcode calls these from various tasks.
//
// Design philosophy: capture semantic signal, not noise. High-frequency
// events (every keystroke, every token) are NOT forwarded — only
// meaningful state transitions.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Event types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionStartEvent {
    pub session_id: String,
    pub project_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserPromptEvent {
    pub session_id: String,
    pub prompt: String,
    pub preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreToolEvent {
    pub tool_name: String,
    pub params_preview: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostToolEvent {
    pub tool_name: String,
    pub result_summary: String,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryWriteEvent {
    pub operation: String,
    pub memory_id: String,
    pub success: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StopEvent {
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmbedderEvent {
    pub model_name: String,
    pub success: bool,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// EventCapture trait
// ---------------------------------------------------------------------------

pub trait EventCapture: Send + Sync + 'static {
    fn on_session_start(&self, event: SessionStartEvent);
    fn on_user_prompt_submit(&self, event: UserPromptEvent);
    fn on_pre_tool_use(&self, event: PreToolEvent);
    fn on_post_tool_use(&self, event: PostToolEvent);
    fn on_memory_write(&self, event: MemoryWriteEvent);
    fn on_stop(&self, event: StopEvent);
    fn on_embedder_ready(&self, event: EmbedderEvent);
}

// ---------------------------------------------------------------------------
// NoOpEventCapture
// ---------------------------------------------------------------------------

pub struct NoOpEventCapture;

impl EventCapture for NoOpEventCapture {
    fn on_session_start(&self, _event: SessionStartEvent) {}
    fn on_user_prompt_submit(&self, _event: UserPromptEvent) {}
    fn on_pre_tool_use(&self, _event: PreToolEvent) {}
    fn on_post_tool_use(&self, _event: PostToolEvent) {}
    fn on_memory_write(&self, _event: MemoryWriteEvent) {}
    fn on_stop(&self, _event: StopEvent) {}
    fn on_embedder_ready(&self, _event: EmbedderEvent) {}
}

// ---------------------------------------------------------------------------
// EventCaptureBox
// ---------------------------------------------------------------------------

pub type EventCaptureBox = Box<dyn EventCapture>;

// ---------------------------------------------------------------------------
// MultiEventCapture — fan-out to multiple listeners
// ---------------------------------------------------------------------------

pub struct MultiEventCapture {
    listeners: std::sync::RwLock<Vec<EventCaptureBox>>,
}

impl MultiEventCapture {
    pub fn new() -> Self {
        Self {
            listeners: std::sync::RwLock::new(Vec::new()),
        }
    }

    pub fn register(&self, listener: EventCaptureBox) {
        self.listeners.write().unwrap().push(listener);
    }
}

impl Default for MultiEventCapture {
    fn default() -> Self {
        Self::new()
    }
}

impl EventCapture for MultiEventCapture {
    fn on_session_start(&self, event: SessionStartEvent) {
        let listeners = self.listeners.read().unwrap();
        for listener in listeners.iter() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                listener.on_session_start(event.clone());
            }));
        }
    }

    fn on_user_prompt_submit(&self, event: UserPromptEvent) {
        let listeners = self.listeners.read().unwrap();
        for listener in listeners.iter() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                listener.on_user_prompt_submit(event.clone());
            }));
        }
    }

    fn on_pre_tool_use(&self, event: PreToolEvent) {
        let listeners = self.listeners.read().unwrap();
        for listener in listeners.iter() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                listener.on_pre_tool_use(event.clone());
            }));
        }
    }

    fn on_post_tool_use(&self, event: PostToolEvent) {
        let listeners = self.listeners.read().unwrap();
        for listener in listeners.iter() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                listener.on_post_tool_use(event.clone());
            }));
        }
    }

    fn on_memory_write(&self, event: MemoryWriteEvent) {
        let listeners = self.listeners.read().unwrap();
        for listener in listeners.iter() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                listener.on_memory_write(event.clone());
            }));
        }
    }

    fn on_stop(&self, event: StopEvent) {
        let listeners = self.listeners.read().unwrap();
        for listener in listeners.iter() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                listener.on_stop(event.clone());
            }));
        }
    }

    fn on_embedder_ready(&self, event: EmbedderEvent) {
        let listeners = self.listeners.read().unwrap();
        for listener in listeners.iter() {
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                listener.on_embedder_ready(event.clone());
            }));
        }
    }
}
