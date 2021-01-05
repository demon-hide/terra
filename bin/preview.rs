use cgmath::EuclideanSpace;
use gilrs::{Axis, Button, Gilrs};
use std::{f64::consts::PI, path::PathBuf};
use structopt::StructOpt;
use winit::{
    event,
    event_loop::{ControlFlow, EventLoop},
};

#[derive(Debug, StructOpt)]
struct Opt {
    #[structopt(short, long, default_value = "8FH495PF+29")]
    plus: String,
    #[structopt(short, long, default_value = "0")]
    heading: f64,
    #[structopt(short, long, default_value = "200000")]
    elevation: f64,
    #[structopt(long)]
    generate: Option<PathBuf>,
}

fn compute_projection_matrix(width: f32, height: f32) -> cgmath::Matrix4<f32> {
    let aspect = width / height;
    let f = 1.0 / (45.0f32.to_radians() / aspect).tan();
    let near = 0.1;

    #[cfg_attr(rustfmt, rustfmt_skip)]
    cgmath::Matrix4::new(
        f/aspect,  0.0,  0.0,   0.0,
        0.0,       f,    0.0,   0.0,
        0.0,       0.0,  0.0,  -1.0,
        0.0,       0.0,  near,  0.0)
}

fn make_swapchain(
    device: &wgpu::Device,
    surface: &wgpu::Surface,
    width: u32,
    height: u32,
) -> wgpu::SwapChain {
    device.create_swap_chain(
        &surface,
        &wgpu::SwapChainDescriptor {
            usage: wgpu::TextureUsage::RENDER_ATTACHMENT,
            format: wgpu::TextureFormat::Bgra8UnormSrgb,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo, // disable vsync by switching to Mailbox,
        },
    )
}
fn make_depth_buffer(device: &wgpu::Device, width: u32, height: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            size: wgpu::Extent3d { width, height, depth: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsage::RENDER_ATTACHMENT,
            label: None,
        })
        .create_view(&Default::default())
}

fn screenshot() {
    let runtime = tokio::runtime::Runtime::new().unwrap();

    let instance = wgpu::Instance::new(wgpu::BackendBit::VULKAN | wgpu::BackendBit::DX12);
    let adapter = runtime
        .block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
        }))
        .expect("Unable to create compatible wgpu adapter");
    let (device, mut queue) = runtime
        .block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                features: wgpu::Features::TEXTURE_COMPRESSION_BC,
                limits: wgpu::Limits::default(),
                label: None,
            },
            None,
        ))
        .expect("Unable to create compatible wgpu device");

    let frame = device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d { width: 1920, height: 1080, depth: 1 },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Bgra8UnormSrgb,
        usage: wgpu::TextureUsage::COPY_SRC | wgpu::TextureUsage::RENDER_ATTACHMENT,
    });

    let opt = Opt::from_args();
    let plus_center =
        open_location_code::decode(&opt.plus).expect("Failed to parse plus code").center;

    let mut depth_buffer = make_depth_buffer(&device, 1920, 1080);
    let mut proj = compute_projection_matrix(1920.0, 1080.0);

    let planet_radius = 6371000.0;
    let mut angle = opt.heading.to_radians();
    let mut lat = plus_center.y().to_radians();
    let mut long = plus_center.x().to_radians();
    let mut altitude = opt.elevation;
    
    let mut terrain = terra::Terrain::new(&device, &mut queue).unwrap();

    {
        let r = altitude + planet_radius;
        let eye = cgmath::Point3::new(
            r * lat.cos() * long.cos(),
            r * lat.cos() * long.sin(),
            r * lat.sin(),
        );
        while terrain.poll_loading_status(&device, &mut queue, eye.into()) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    let surface_height = terrain.get_height(lat, long) as f64;
    let r = altitude + planet_radius + surface_height + 2.0;
    let eye = cgmath::Point3::new(
        r * lat.cos() * long.cos(),
        r * lat.cos() * long.sin(),
        r * lat.sin(),
    );

    let dt = (planet_radius / (planet_radius + altitude)).acos() * 0.3;
    let latc = lat + angle.cos() * dt;
    let longc = long - angle.sin() * dt;

    let center = cgmath::Point3::new(
        planet_radius * latc.cos() * longc.cos() - eye.x,
        planet_radius * latc.cos() * longc.sin() - eye.y,
        planet_radius * latc.sin() - eye.z,
    );
    let up = cgmath::Vector3::new(eye.x as f32, eye.y as f32, eye.z as f32);

    let view = cgmath::Matrix4::look_at_rh(
        cgmath::Point3::origin(),
        cgmath::Point3::new(center.x as f32, center.y as f32, center.z as f32),
        up,
    );

    let view_proj = proj * view;
    let view_proj = mint::ColumnMatrix4 {
        x: view_proj.x.into(),
        y: view_proj.y.into(),
        z: view_proj.z.into(),
        w: view_proj.w.into(),
    };

    terrain.render(
        &device,
        &mut queue,
        &frame.create_view(&Default::default()),
        &depth_buffer,
        (1920, 1080),
        view_proj,
        eye.into(),
    );

    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 2048 * 1080 * 4,
        usage: wgpu::BufferUsage::COPY_DST | wgpu::BufferUsage::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(wgpu::TextureCopyView {
        texture: &frame,
        mip_level: 0,
        origin: wgpu::Origin3d::ZERO,
    }, wgpu::BufferCopyView {buffer: &buffer, layout: wgpu::TextureDataLayout {
        offset: 0,
        bytes_per_row: 2048 * 4,
        rows_per_image: 0,
    } }, wgpu::Extent3d {width: 1920, height: 1080, depth: 1});
    queue.submit(Some(encoder.finish()));
    
    let fut = buffer.slice(..).map_async(wgpu::MapMode::Read);
    device.poll(wgpu::Maintain::Wait);
    runtime.block_on(fut);

    let mut img = Vec::new();
    for row in buffer.slice(..).get_mapped_range().chunks(2048 * 4) {
        img.extend_from_slice(&row[..1920*4]);
    }
    for pixel in img.chunks_mut(4) {
        let (r, b) = (pixel[2], pixel[0]);
        pixel[0] = r;
        pixel[2] = b;
        pixel[3] = 255;
    }

    image::save_buffer("capture.png", &img, 1920, 1080, image::ColorType::Rgba8).unwrap();
}

fn main() {
    // env_logger::init();
    screenshot();
    return;

    let runtime = tokio::runtime::Runtime::new().unwrap();

    let trace_path: Option<&std::path::Path> = if cfg!(feature = "trace") {
        std::fs::create_dir_all("trace").unwrap();
        Some(std::path::Path::new("trace"))
    } else {
        None
    };

    let event_loop = EventLoop::new();
    let monitor = event_loop.available_monitors()
        .find(|monitor| monitor.video_modes().any(|mode| mode.size().width == 1920));
    let window = winit::window::WindowBuilder::new()
        .with_visible(false)
        .with_fullscreen(Some(winit::window::Fullscreen::Borderless(monitor)))
        .build(&event_loop)
        .unwrap();

    let instance = wgpu::Instance::new(wgpu::BackendBit::VULKAN | wgpu::BackendBit::DX12);
    let surface = unsafe { instance.create_surface(&window) };
    let adapter = runtime
        .block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
        }))
        .expect("Unable to create compatible wgpu adapter");
    let (device, mut queue) = runtime
        .block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                features: wgpu::Features::TEXTURE_COMPRESSION_BC,
                limits: wgpu::Limits::default(),
                label: None,
            },
            trace_path,
        ))
        .expect("Unable to create compatible wgpu device");

    let mut size = window.inner_size();
    let mut swap_chain = make_swapchain(&device, &surface, size.width, size.height);
    let mut depth_buffer = make_depth_buffer(&device, size.width, size.height);
    let mut proj = compute_projection_matrix(size.width as f32, size.height as f32);

    let mut gilrs = Gilrs::new().unwrap();
    let mut current_gamepad = None;
    for (_id, gamepad) in gilrs.gamepads() {
        current_gamepad = Some(gamepad.id());
    }

    let opt = Opt::from_args();
    let plus_center =
        open_location_code::decode(&opt.plus).expect("Failed to parse plus code").center;

    let planet_radius = 6371000.0;
    let mut angle = opt.heading.to_radians();
    let mut lat = plus_center.y().to_radians();
    let mut long = plus_center.x().to_radians();
    let mut altitude = opt.elevation;

    let mut terrain = terra::Terrain::new(&device, &mut queue).unwrap();

    if let Some(dataset_directory) = opt.generate {
        let pb = indicatif::ProgressBar::new(100);
        pb.set_style(indicatif::ProgressStyle::default_bar()
            .template("{msg} {pos}/{len} [{wide_bar}] {percent}% {per_sec} {eta}")
            .progress_chars("=> "));
        let mut last_message = None;
        let mut progress_callback = |l: &str, i: usize, total: usize|{
            if last_message.is_none() || l != last_message.as_ref().unwrap() {
                pb.set_message(l);
                pb.reset_eta();
                last_message = Some(l.to_string());
            }
            pb.set_length(total as u64);
            pb.set_position(i as u64);
        };

        runtime.block_on(terrain.generate_heightmaps(
            dataset_directory.join("ETOPO1_Ice_c_geotiff.zip"),
            dataset_directory.join("strm3"), 
            &mut progress_callback)).unwrap();
        runtime.block_on(terrain.generate_albedos(dataset_directory.join("bluemarble"), &mut progress_callback)).unwrap();
        runtime.block_on(terrain.generate_roughness(&mut progress_callback)).unwrap();
    }

    {
        let r = altitude + planet_radius;
        let eye = cgmath::Point3::new(
            r * lat.cos() * long.cos(),
            r * lat.cos() * long.sin(),
            r * lat.sin(),
        );
        while terrain.poll_loading_status(&device, &mut queue, eye.into()) {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    let mut set_visible = false;
    event_loop.run(move |event, _, control_flow| {
        *control_flow = if cfg!(feature = "metal-auto-capture") {
            ControlFlow::Exit
        } else {
            ControlFlow::Poll
        };
        match event {
            event::Event::WindowEvent { event, .. } => match event {
                event::WindowEvent::CloseRequested => {
                    *control_flow = ControlFlow::Exit;
                }
                event::WindowEvent::KeyboardInput {
                    input:
                        event::KeyboardInput {
                            virtual_keycode: Some(keycode),
                            state: event::ElementState::Pressed,
                            ..
                        },
                    ..
                } => match keycode {
                    event::VirtualKeyCode::Escape => *control_flow = ControlFlow::Exit,
                    event::VirtualKeyCode::Space => altitude += 0.1 * altitude,
                    event::VirtualKeyCode::Semicolon => altitude -= 0.1 * altitude,
                    event::VirtualKeyCode::Left => {
                        lat += -angle.sin() * -(0.0000001 * altitude).min(0.01);
                        long += angle.cos() * -(0.0000001 * altitude).min(0.01);
                    }
                    event::VirtualKeyCode::Right => {
                        lat += -angle.sin() * (0.0000001 * altitude).min(0.01);
                        long += angle.cos() * (0.0000001 * altitude).min(0.01);
                    }
                    event::VirtualKeyCode::Up => {
                        lat += angle.cos() * (0.0000001 * altitude).min(0.01);
                        long += -angle.sin() * (0.0000001 * altitude).min(0.01);
                    }
                    event::VirtualKeyCode::Down => {
                        lat += angle.cos() * -(0.0000001 * altitude).min(0.01);
                        long += -angle.sin() * -(0.0000001 * altitude).min(0.01);
                    }
                    _ => {}
                },
                event::WindowEvent::Resized(new_size) => {
                    size = new_size;
                    swap_chain = make_swapchain(&device, &surface, size.width, size.height);
                    depth_buffer = make_depth_buffer(&device, size.width, size.height);
                    proj = compute_projection_matrix(size.width as f32, size.height as f32);
                }
                _ => {}
            },
            event::Event::MainEventsCleared => {
                let frame = swap_chain.get_current_frame();
                let frame = match frame {
                    Ok(ref f) => &f.output.view,
                    Err(_) => return,
                };

                while let Some(gilrs::Event { id, event: _event, time: _ }) = gilrs.next_event() {
                    current_gamepad = Some(id);
                }
                if let Some(gamepad) = current_gamepad.map(|id| gilrs.gamepad(id)) {
                    lat += angle.cos() * gamepad.value(Axis::LeftStickY) as f64 * 0.01
                        - angle.sin() * gamepad.value(Axis::LeftStickX) as f64 * 0.01;

                    long += -angle.sin() * gamepad.value(Axis::LeftStickY) as f64 * 0.01
                        + angle.cos() * gamepad.value(Axis::LeftStickX) as f64 * 0.01;

                    angle -= gamepad.value(Axis::RightZ) as f64 * 0.01;

                    if gamepad.is_pressed(Button::DPadUp) {
                        altitude += 0.01 * altitude;
                    }
                    if gamepad.is_pressed(Button::DPadDown) {
                        altitude -= 0.01 * altitude;
                    }
                }

                lat = lat.max(-PI).min(PI);
                if long < -PI {
                    long += PI * 2.0;
                }
                if long > PI {
                    long -= PI * 2.0;
                }

                let surface_height = terrain.get_height(lat, long) as f64;
                let r = altitude + planet_radius + surface_height + 2.0;
                let eye = cgmath::Point3::new(
                    r * lat.cos() * long.cos(),
                    r * lat.cos() * long.sin(),
                    r * lat.sin(),
                );

                let dt = (planet_radius / (planet_radius + altitude)).acos() * 0.3;
                let latc = lat + angle.cos() * dt;
                let longc = long - angle.sin() * dt;

                let center = cgmath::Point3::new(
                    planet_radius * latc.cos() * longc.cos() - eye.x,
                    planet_radius * latc.cos() * longc.sin() - eye.y,
                    planet_radius * latc.sin() - eye.z,
                );
                let up = cgmath::Vector3::new(eye.x as f32, eye.y as f32, eye.z as f32);

                let view = cgmath::Matrix4::look_at_rh(
                    cgmath::Point3::origin(),
                    cgmath::Point3::new(center.x as f32, center.y as f32, center.z as f32),
                    up,
                );

                let view_proj = proj * view;
                let view_proj = mint::ColumnMatrix4 {
                    x: view_proj.x.into(),
                    y: view_proj.y.into(),
                    z: view_proj.z.into(),
                    w: view_proj.w.into(),
                };

                terrain.render(
                    &device,
                    &mut queue,
                    &frame,
                    &depth_buffer,
                    (size.width, size.height),
                    view_proj,
                    eye.into(),
                );

                if !set_visible {
                    window.set_visible(true);
                    set_visible = true;
                }
            }
            _ => (),
        }
    });
}
