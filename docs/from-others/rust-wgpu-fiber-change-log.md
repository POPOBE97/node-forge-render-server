## Change log

- 2026-01-06：新增 runtime API（`write_buffer*`/`buffer_info`/`texture_info`/`write_texture_rgba8`）与结构化错误 `ShaderSpaceError`
- 2026-01-06：新增 `dump_summary()`，并在 rust-wgpu-fiber.md 补充 runtime update 用法
- 2026-01-06：新增批量声明入口：`declare_buffers`/`declare_textures`/`declare_samplers`
- 2026-01-06：owned name 全库落地：引入 `ResourceName(Arc<str>)`，移除 `ShaderSpace<'a>` lifetime，统一资源 key 与声明/构建入口