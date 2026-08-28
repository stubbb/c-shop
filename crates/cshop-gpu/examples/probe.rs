fn main() {
    env_logger::init();
    match cshop_gpu::context::GpuContext::headless() {
        Ok(ctx) => {
            let info = ctx.adapter.get_info();
            println!("adapter : {}", info.name);
            println!("backend : {:?}", info.backend);
            println!("type    : {:?}", info.device_type);
            println!("max 2D  : {}", ctx.max_texture_dim());
        }
        Err(e) => println!("FAILED: {e}"),
    }
}
