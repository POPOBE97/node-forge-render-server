//! Bounded, thread-owned JavaScript execution for native mobile playback.
use std::{cell::Cell, rc::Rc, time::{Duration, Instant}};

use anyhow::{Result, anyhow};
use rquickjs::{Context, Ctx, Function, Runtime, Value};

pub(crate) struct PortableJs {
    context: Context,
    _runtime: Runtime,
    deadline: Rc<Cell<Option<Instant>>>,
}

fn error(ctx: &Ctx<'_>, error: rquickjs::Error) -> anyhow::Error {
    if error.is_exception() {
        let value = ctx.catch();
        return anyhow!("JavaScript exception: {}", value.as_exception()
            .and_then(|exception| exception.message()).unwrap_or_else(|| format!("{value:?}")));
    }
    anyhow!("JavaScript: {error}")
}

impl PortableJs {
    pub(crate) fn new() -> Result<Self> {
        let runtime = Runtime::new()?;
        runtime.set_memory_limit(64 * 1024 * 1024);
        runtime.set_max_stack_size(512 * 1024);
        let deadline = Rc::new(Cell::new(None::<Instant>));
        let interrupt_deadline = deadline.clone();
        runtime.set_interrupt_handler(Some(Box::new(move || {
            interrupt_deadline.get().is_some_and(|deadline| Instant::now() >= deadline)
        })));
        let context = Context::full(&runtime)?;
        Ok(Self { context, _runtime: runtime, deadline })
    }

    pub(crate) fn install(&self, source: &str) -> Result<()> {
        self.deadline.set(Some(Instant::now() + Duration::from_millis(250)));
        let result = self.context.with(|ctx| {
            ctx.eval::<(), _>(source).map_err(|err| error(&ctx, err))
        });
        self.deadline.set(None);
        result
    }

    pub(crate) fn call(&self, input: &str, budget: Duration) -> Result<String> {
        if budget.is_zero() { anyhow::bail!("Graph Function exceeded the graph frame budget"); }
        self.deadline.set(Some(Instant::now() + budget));
        let result = self.context.with(|ctx| {
            let function: Function = ctx.globals().get("__nodeForgeInvoke")
                .map_err(|err| error(&ctx, err))?;
            function.call::<_, String>((input,)).map_err(|err| error(&ctx, err))
        });
        self.deadline.set(None);
        result
    }

    pub(crate) fn eval_json<T: serde::de::DeserializeOwned>(&self, source: &str) -> Result<T> {
        self.deadline.set(Some(Instant::now() + Duration::from_secs(1)));
        let result = self.context.with(|ctx| {
            let value: Value = ctx.eval(source).map_err(|err| error(&ctx, err))?;
            let json = ctx.json_stringify(value).map_err(|err| error(&ctx, err))?
                .ok_or_else(|| anyhow!("DataParse returned undefined"))?
                .to_string().map_err(|err| error(&ctx, err))?;
            serde_json::from_str(&json).map_err(Into::into)
        });
        self.deadline.set(None);
        result
    }
}
