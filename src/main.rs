use anyhow::Error;
use gstreamer::{glib::{self, value::ToValue}, prelude::*, ClockTime, DebugGraphDetails, ElementFactory};
use gstreamer as gst; // Add this line
use glob::glob;
use strum_macros::Display;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;
use std::sync::{Arc, Mutex};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use tracing::{info, warn, error, debug, trace};
use tracing_subscriber;


struct PipelineManager {
    pipelines: HashMap<String, gstreamer::Pipeline>,
    dot_path: String,
    network_interface: String,
}

impl PipelineManager {
    fn new(network_interface: String) -> Self {
        Self {
            pipelines: HashMap::new(),
            dot_path: "./dot_path".to_string(),
            network_interface,
        }
    }

    fn create_pipeline(&mut self, name: &str, description: &str) -> Result<(), Error> {
        if self.pipelines.contains_key(name) {
            warn!("Pipeline '{}' already exists.", name);
            return Ok(());
        }

      let mut ct = gstreamer::ClockType::Tai; //default TAI
        if let Ok(clocktype) = std::env::var("GSTR_DEFAULT_CLOCK_TYPE") {
            match clocktype.to_lowercase().trim() {
                "tai" => {
                    ct = gstreamer::ClockType::Tai;
                }
                "realtime" => {
                    ct = gstreamer::ClockType::Realtime;
                }
                "monotonic" => {
                    ct = gstreamer::ClockType::Monotonic;
                }
                _ => {
                    error!(
                        "unknown clock type: '{clocktype}'. clock type should be 'tai', 'realtime' or 'monotonic'"
                    );
                    panic!("unknown clock type specified");
                }
            }
        }

        debug!("set default clock type: {:?}", ct);
        let clock = gstreamer::SystemClock::obtain()
            .dynamic_cast::<gstreamer::SystemClock>()
            .unwrap();
        clock.set_clock_type(ct);
         

        let pipeline = gstreamer::parse::launch(description)?
            .downcast::<gstreamer::Pipeline>()
            .expect("Expected a pipeline");

        pipeline.set_start_time(ClockTime::NONE);
        pipeline.set_base_time(ClockTime::ZERO);
        
//        let bin = pipeline.upcast::<gstreamer::Bin>();
        for element in pipeline.iterate_elements() {
            let element = element?;
            if let Some(factory) = ElementFactory::find(&element.factory().map(|f|f.name()).unwrap_or_default()) {
                if factory.name() == "sdpdemux" {
                    let pspecs = element.list_properties();
                    for pspec in pspecs {
                        trace!("{}",pspec.name());
                    }
//                    debug!("{:?}",element.pr);        // rtcp-mode
                    if element.has_property("rtcp-mode") {
                        debug!( "Found rtcp-mode propety on sdpdemux element.");
                        element.set_property_from_str("rtcp-mode", "0");
                    } else {
                        debug!( "Could not find rtcp-mode propety on sdpdemux element.");
                    }
                    
                    if element.has_property("timeout-inactive-rtp-sources") {
                        debug!( "Found timeout-inactive-rtp-sources propety on sdpdemux element.");
                        element.set_property_from_str("timeout-inactive-rtp-sources", "FALSE");
                    } else {
                        debug!( "Could not find timeout-inactive-rtp-sources propety on sdpdemux element.");
                    }
                    let pipeline_clone = pipeline.clone();
                    if let Err(e) = add_input_handlers(
                        &pipeline_clone.upcast::<gstreamer::Bin>(),
                        &element,
                    ) {
                        warn!("⚠️ Failed to add input handlers: {}", e);
                    } else {
                        info!("✅ Added SSRC switching handlers to sdpdemux");
                    }


                    // Connect to element-added signal to configure udpsrc elements
                    let _pipeline_weak = pipeline.downgrade();
                    let interface = self.network_interface.clone();
                    element.connect("element-added", false, move |values| {
                        if let (Ok(_sdpdemux), Ok(child_element)) = (
                            values[0].get::<gstreamer::Element>(),
                            values[1].get::<gstreamer::Element>()
                        ) {
                            if let Some(factory) = child_element.factory() {
                                if factory.name() == "udpsrc" {
                                    if child_element.has_property("multicast-iface") {
                                        child_element.set_property_from_str("multicast-iface", &interface);
                                        info!("🌐 Set multicast-iface={} on udpsrc element", interface);
                                    }
                                    // Also set socket property for better multicast handling
                                    if child_element.has_property("reuse") {
                                        child_element.set_property("reuse", &true);
                                        debug!("🔄 Set reuse=true on udpsrc element");
                                    }
                                }
                            }
                        }
                        None
                    });
                }

            }

        }

        // Set up bus message handling
        let bus = pipeline.bus().expect("Pipeline should have a bus");
        let pipeline_name = name.to_string();
        let pipeline_element = pipeline.clone();
        
        bus.set_sync_handler(move |_bus, msg| {
            use gstreamer::MessageView;
            
            match msg.view() {
                MessageView::Error(err) => {
                    error!("🚨 ERROR on pipeline {}: {} ({})", 
                           pipeline_name, 
                           err.error(), 
                           err.debug().unwrap_or_default());
                }
                MessageView::Warning(warning) => {
                    warn!("⚠️ WARNING on pipeline {}: {} ({})", 
                           pipeline_name, 
                           warning.error(), 
                           warning.debug().unwrap_or_default());
                }
                MessageView::Eos(_) => {
                    info!("🔚 End of stream on pipeline {}", pipeline_name);
                }
                MessageView::StateChanged(state_changed) => {
                    // Only log state changes for the pipeline itself, not individual elements
                    if msg.src() == Some(pipeline_element.upcast_ref()) {
                        info!("🔄 Pipeline {} state changed: {:?} -> {:?}", 
                               pipeline_name,
                               state_changed.old(),
                               state_changed.current());
                    }
                }
                MessageView::StreamStart(_) => {
                    info!("🚀 Stream started on pipeline {}", pipeline_name);
                }
                MessageView::Latency(_) => {
                    // Handle latency messages silently to avoid spam
                }
                MessageView::Tag(_) | MessageView::Toc(_) => {
                    // Handle metadata messages silently
                }
                _ => {
                    // Handle all other messages silently to prevent spam
                    // This includes: Buffering, Duration, AsyncDone, etc.
                }
            }
            
            // Always drop the message to prevent memory accumulation
            gstreamer::BusSyncReply::Drop
        });

        self.pipelines.insert(name.to_string(), pipeline);
        info!("✅ Created pipeline: {}", name);
        Ok(())
    }

    fn start_pipeline(&self, name: &str) -> Result<(), Error> {
        match self.pipelines.get(name) {
            Some(pipeline) => {
                let current_state = pipeline.current_state();
                if current_state == gstreamer::State::Playing {
                    warn!("⚠️ Pipeline '{}' is already playing.", name);
                } else {
                    pipeline.set_state(gstreamer::State::Playing)?;
                    info!("▶️ Started pipeline: {}", name);
                }
            }
            None => error!("❌ Pipeline '{}' not found.", name),
        }
        Ok(())
    }

    fn stop_pipeline(&self, name: &str) -> Result<(), Error> {
        match self.pipelines.get(name) {
            Some(pipeline) => {
                let current_state = pipeline.current_state();
                if current_state == gstreamer::State::Null {
                    warn!("⚠️ Pipeline '{}' is already stopped.", name);
                } else {
                    // Clean up any ghost pads in sdpdemux before stopping
                    for elem in pipeline.iterate_elements() {
                        if let Ok(elem) = elem {
                            if elem.type_().name() == "GstSDPDemux" {
                                let sdpdemux = elem.dynamic_cast::<gst::Bin>().unwrap();
                                
                                // Collect pads to remove (avoid iterator invalidation)
                                let pads_to_remove: Vec<_> = sdpdemux.src_pads().iter()
                                    .filter(|pad| pad.name() == "stream_0")
                                    .cloned()
                                    .collect();
                                
                                for pad in pads_to_remove {
                                    debug!("🧹 DEBUG: Cleaning up {} stream_0 ghost pad on pipeline stop", sdpdemux.name());
                                    // Set pad to inactive before removal to avoid assertion errors
                                    if let Err(err) = pad.set_active(false) {
                                        debug!("⚠️ DEBUG: Error deactivating pad (may be expected): {}", err);
                                    }
                                    if let Err(err) = sdpdemux.remove_pad(&pad) {
                                        debug!("⚠️ DEBUG: Error removing stream_0 pad during stop (may be expected): {}", err);
                                    }
                                }
                            }
                        }
                    }
                    
                    pipeline.set_state(gstreamer::State::Null)?;
                    info!("⏹️ Stopped pipeline: {}", name);
                }
            }
            None => error!("❌ Pipeline '{}' not found.", name),
        }
        Ok(())
    }

    fn remove_pipeline(&mut self, name: &str) -> Result<(), Error> {
        match self.pipelines.remove(name) {
            Some(pipeline) => {
                pipeline.set_state(gstreamer::State::Null)?;
                info!("🗑️ Removed pipeline: {}", name);
            }
            None => error!("❌ Pipeline '{}' not found.", name),
        }
        Ok(())
    }

    fn toggle_pipeline(&self, name: &str, timeout_secs: u64) -> Result<(), Error> {
        match self.pipelines.get(name) {
            Some(pipeline) => {
                let current_state = pipeline.current_state();
                
                if current_state == gstreamer::State::Playing {
                    // Stop the pipeline
                    pipeline.set_state(gstreamer::State::Null)?;
                    info!("⏹️ Stopped pipeline: {}", name);
                    
                    if timeout_secs > 0 {
                        info!("⏱️ Waiting {} seconds before restarting...", timeout_secs);
                        std::thread::sleep(std::time::Duration::from_secs(timeout_secs));
                    }
                    
                    // Start the pipeline again
                    pipeline.set_state(gstreamer::State::Playing)?;
                    info!("▶️ Restarted pipeline: {}", name);
                } else {
                    // If not playing, just start it
                    pipeline.set_state(gstreamer::State::Playing)?;
                    info!("▶️ Started pipeline: {}", name);
                }
            }
            None => error!("❌ Pipeline '{}' not found.", name),
        }
        Ok(())
    }

    fn list_pipelines(&self) {
        if self.pipelines.is_empty() {
            info!("📭 No pipelines available.");
        } else {
            info!("📋 Available pipelines:");
            for (name, pipeline) in &self.pipelines {
                let state = pipeline.current_state();
                let latency_info = if state == gstreamer::State::Playing {
                    // Query latency using a latency query
                    let mut query = gstreamer::query::Latency::new();
                    if pipeline.query(&mut query) {
                        let (live, min_latency, max_latency) = query.result();
                        let min_ms = min_latency.mseconds();
                        let max_ms = max_latency.map(|t| t.mseconds()).unwrap_or(0);
                        format!(" (latency: {}ms-{}ms, live: {})", min_ms, max_ms, live)
                    } else {
                        " (latency: query failed)".to_string()
                    }
                } else {
                    String::new()
                };
                info!(" - {} [{}]{}", name, format!("{:?}", state), latency_info);
                // // Dump DOT file
                // debug_bin_to_dot_file_with_ts(
                //     pipeline,
                //     DebugGraphDetails::all(),
                //     &format!("{}_graph", name),
                // );
                let _ = self.debug_pipeline(name);
            }
        }
    }

    fn show_latency_info(&self) {
        if self.pipelines.is_empty() {
            info!("📭 No pipelines available.");
        } else {
            info!("⏱️ Pipeline Latency Details:");
            for (name, pipeline) in &self.pipelines {
                let state = pipeline.current_state();
                info!("  📊 Pipeline: {} [{}]", name, format!("{:?}", state));
                
                if state == gstreamer::State::Playing {
                    // Query latency using a latency query
                    let mut query = gstreamer::query::Latency::new();
                    if pipeline.query(&mut query) {
                        let (live, min_latency, max_latency) = query.result();
                        let min_ms = min_latency.mseconds();
                        let max_ms = max_latency.map(|t| t.mseconds()).unwrap_or(0);
                        let min_ns = min_latency.nseconds();
                        let max_ns = max_latency.map(|t| t.nseconds()).unwrap_or(0);
                        
                        info!("     🎯 Live pipeline: {}", live);
                        info!("     ⏰ Min latency: {}ms ({}ns)", min_ms, min_ns);
                        info!("     ⏰ Max latency: {}ms ({}ns)", max_ms, max_ns);
                        
                        if live {
                            info!("     ℹ️  This is a live pipeline with real-time constraints");
                        } else {
                            info!("     ℹ️  This is a non-live pipeline (file-based)");
                        }
                    } else {
                        warn!("     ❌ Could not query latency information");
                    }
                } else {
                    info!("     ⏸️  Pipeline not playing - latency info unavailable");
                }
                info!("");
            }
        }
    }


    fn debug_pipeline(&self, name: &str) -> Result<(), Error> {
        if let Some(pipeline) = self.pipelines.get(name) {
            // Create dot directory if it doesn't exist
            fs::create_dir_all(&self.dot_path)?;
            
            // Clean up old DOT and PNG files for this pipeline
            let dot_pattern = format!("{}/*{}*.dot", self.dot_path, name);
            let png_pattern = format!("{}/*{}*.png", self.dot_path, name);
            
            // Remove old DOT files
            if let Ok(dot_paths) = glob(&dot_pattern) {
                for path in dot_paths.filter_map(Result::ok) {
                    if let Err(e) = fs::remove_file(&path) {
                        warn!("⚠️ Failed to remove old DOT file {:?}: {}", path, e);
                    } else {
                        debug!("🗑️ Removed old DOT file: {:?}", path);
                    }
                }
            }
            
            // Remove old PNG files
            if let Ok(png_paths) = glob(&png_pattern) {
                for path in png_paths.filter_map(Result::ok) {
                    if let Err(e) = fs::remove_file(&path) {
                        warn!("⚠️ Failed to remove old PNG file {:?}: {}", path, e);
                    } else {
                        debug!("🗑️ Removed old PNG file: {:?}", path);
                    }
                }
            }
            
            // Generate DOT data using the available Rust method
            let dot_data = pipeline.debug_to_dot_data(DebugGraphDetails::all());
            
            // Create filename with timestamp
            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();
            
            let filename = format!("{}_{}.dot", name, timestamp);
            let filepath = Path::new(&self.dot_path).join(filename);
            
            // Write DOT data to file
            fs::write(&filepath, dot_data)?;
            
            debug!("✅ Debug DOT file generated: {:?}", filepath);
            
            // Generate PNG file
            let png_path = format!("{}/{}_{}.png", self.dot_path, name, timestamp);
            convert_dot_to_png(filepath.to_str().unwrap(), &png_path);
            debug!("✅ Debug PNG file generated: {}", png_path);
        }
        Ok(())
    }    
}


fn convert_dot_to_png(dot_path: &str, png_path: &str) {
    let result = Command::new("dot")
        .args(["-Tpng", dot_path, "-o", png_path])
        .output();

    match result {
        Ok(output) if output.status.success() => {
            debug!("✅ PNG generated at: {}", png_path);
        }
        Ok(output) => {
            error!(
                "❌ Failed to generate PNG: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Err(e) => {
            error!("❌ Error running dot command: {}", e);
        }
    }
}

/*
this will add handlers for rtpjitterbuffer settings
also there is special handling for switching ssrc from source
*/
#[derive(Default, Debug, Clone, Eq, PartialEq)]
pub enum RFC7273Sync {
    #[default]
    False,
    True,
    UseSystemClock,
}

#[derive(Default, Debug, Clone, Eq, PartialEq)]
pub enum RFC7273ReferenceTimestamp {
    ReferenceTimestamp,
    MetaOnly,
    #[default]
    None,
}


#[derive(
    Default, Debug, Clone, Eq, PartialEq, Display,
)]
pub enum JitterBufferMode {
    None,
    Slave,
    Buffer,
    #[default]
    Synced,
}
struct SsrcId {
    session_id: u32,
    ssrc: u32,
}

fn add_input_handlers(
    in_bin: &gst::Bin,
    sdpdemux: &gst::Element,
) -> Result<(), String> {
    let rfc7273_sync = RFC7273Sync::default();
    let rfc7273_meta = RFC7273ReferenceTimestamp::default();
    let jb_mode = JitterBufferMode::default();

    in_bin.connect_closure("deep-element-added", false,
        glib::closure!(move |_: &gst::Element, _bin: &gst::Element, elem: &gst::Element| {
            if elem.type_().name() == "GstRtpJitterBuffer" {
                if rfc7273_sync == RFC7273Sync::True || rfc7273_sync == RFC7273Sync::UseSystemClock {
                    if rfc7273_sync == RFC7273Sync::UseSystemClock {
                        elem.set_property_from_str("rfc7273-use-system-clock", "true");
                    }

                    elem.set_property_from_str("rfc7273-sync", "true");
                }

				if rfc7273_meta == RFC7273ReferenceTimestamp::MetaOnly {
                    elem.set_property_from_str("rfc7273-reference-timestamp-meta-only", "true");
                }
            }

            if elem.type_().name() == "GstRtpBin" {

                let mode = format!("{jb_mode}").to_lowercase();
                elem.set_property_from_str("buffer-mode", &mode);

                //elem.set_property_from_str("drop-on-latency", "true"); //do not do this, i think. 
                //elem.set_property_from_str("autoremove", "true");

                if rfc7273_sync == RFC7273Sync::True || rfc7273_sync == RFC7273Sync::UseSystemClock {
                    elem.set_property_from_str("rfc7273-sync", "true");
                }
                if rfc7273_meta == RFC7273ReferenceTimestamp::ReferenceTimestamp || rfc7273_meta == RFC7273ReferenceTimestamp::MetaOnly {
                    elem.set_property_from_str("add-reference-timestamp-meta", "true");
                }

                let ssrc_holder = Arc::new(Mutex::new(None::<SsrcId>));
                
                // Connect a SINGLE persistent pad-added handler to the rtpbin
                let demux_bin = elem.parent().unwrap().dynamic_cast::<gst::Bin>().unwrap();
                let demux_bin_clone = demux_bin.clone();
                let ssrc_holder_clone = ssrc_holder.clone();
                
                let _handler_id = elem.connect_closure(
                    "pad-added",
                    false,
                    glib::closure!(move |bin: &gst::Element, pad: &gst::Pad| {
                        trace!("🔗 DEBUG: Pad {} added in {} (checking SSRC match)", pad.name(), bin.name());

                        if let Ok(ssrc_lock) = ssrc_holder_clone.lock() {
                            if let Some(current_ssrc) = &*ssrc_lock {
                                if pad.name().starts_with(&format!("recv_rtp_src_{}_{}", current_ssrc.session_id, current_ssrc.ssrc)) {
                                    debug!("🆕 DEBUG: Creating new stream_0 ghost pad for current SSRC");

                                    // First, clean up any existing stream_0 ghost pad
                                    let existing_pads: Vec<_> = demux_bin_clone.src_pads().iter()
                                        .filter(|pad| pad.name() == "stream_0")
                                        .cloned()
                                        .collect();
                                    
                                    for existing_pad in existing_pads {
                                        debug!("🧹 DEBUG: Removing existing stream_0 ghost pad before creating new one");
                                        // Deactivate before removal to avoid assertion errors
                                        let _ = existing_pad.set_active(false);
                                        if let Err(err) = demux_bin_clone.remove_pad(&existing_pad) {
                                            debug!("⚠️ DEBUG: Error removing existing stream_0 pad (may be expected): {}", err);
                                        }
                                    }

                                    match gst::GhostPad::builder_with_target(pad) {
                                        Ok(gpad) => {
                                            let gpad = gpad.name("stream_0").build();
                                            if let Err(err) = demux_bin_clone.add_pad(&gpad) {
                                                warn!("❌ DEBUG: Error adding ghost pad: {}", err);
                                            } else {
                                                debug!("✅ DEBUG: Added ghost pad for stream_0");
                                            }
                                        },
                                        Err(_err) => {
                                            // Silent error - older GStreamer versions handle this automatically
                                        }
                                    }
                                } else {
                                    trace!("🔍 DEBUG: Pad {} doesn't match current SSRC pattern recv_rtp_src_{}_{}", 
                                           pad.name(), current_ssrc.session_id, current_ssrc.ssrc);
                                }
                            } else {
                                trace!("🔍 DEBUG: No current SSRC stored yet, ignoring pad {}", pad.name());
                            }
                        }
                    }),
                );

                elem.connect_closure("on-new-ssrc", false, 
                    glib::closure!(|bin: &gst::Element, session_id: u32, ssrc: u32| {

                        debug!("🔄 DEBUG: on-new-ssrc - session_id: {session_id}, ssrc: {ssrc} (bin: {})", bin.name());

                        match ssrc_holder.lock() {
                            Ok(mut st) => match &mut *st {
                                st @ None => {
                                    debug!("✅ DEBUG: First stream. saving session_id: {session_id}, {ssrc}");
                                    *st = Some(SsrcId { session_id, ssrc });
                                }
                                Some(st) => {
                                    let old_session_id = st.session_id;
                                    let old_ssrc = st.ssrc;

                                    if session_id == old_session_id {
                                        debug!("🔄 DEBUG: New SSRC detected! Clearing old session_id: {old_session_id}, {old_ssrc}");
                                        bin.emit_by_name_with_values("clear-ssrc", &[old_session_id.to_value(), old_ssrc.to_value()]);

                                        let demux_bin = bin.parent().unwrap().dynamic_cast::<gst::Bin>().unwrap();

                                        for pad in demux_bin.src_pads().iter() {
                                            if pad.name() == "stream_0" {
                                                debug!("🗑️ DEBUG: Removing {} stream_0 pad", demux_bin.name());
                                                if let Err(err) = demux_bin.remove_pad(pad) {
                                                    warn!("❌ Error removing {} stream_0 pad: {}", demux_bin.name(), err);
                                                }
                                            }
                                        }

                                        debug!("💾 DEBUG: Saving new session_id: {session_id}, {ssrc}");
                                        st.session_id = session_id;
                                        st.ssrc = ssrc;
                                    } else {
                                        warn!("⚠️ Got new-ssrc on different session_id {session_id}. How to handle this?");
                                    }
                                }
                            }
                            Err(err) => {
                                error!("err: {err:?}");
                            }
                        }
                    })
                );
            }
        }),
    );
    let mut decode_bin_sink_pad = None;
    
    // Look for decodebin element
    for element in in_bin.iterate_elements() {
        if let Ok(element) = element {
            if let Some(factory) = element.factory() {
                if factory.name() == "decodebin" {
                    if let Some(sink_pad) = element.static_pad("sink") {
                        decode_bin_sink_pad = Some(sink_pad);
                        
                        // Add decodebin pad-added handler for dynamic pad reconnection
                        let pipeline_name = in_bin.name().to_string();
                        let pipeline_name_clone = pipeline_name.clone();
                        let in_bin_clone = in_bin.clone();
                        element.connect_closure(
                            "pad-added",
                            false,
                            glib::closure!(move |decodebin: &gst::Element, pad: &gst::Pad| {
                                debug!("🔗 DEBUG: {} decodebin pad-added: {} in {}", pipeline_name_clone, pad.name(), decodebin.name());
                                
                                // Find audioconvert element in the pipeline
                                for elem in in_bin_clone.iterate_elements() {
                                    if let Ok(elem) = elem {
                                        if let Some(factory) = elem.factory() {
                                            if factory.name() == "audioconvert" {
                                                if let Some(sink_pad) = elem.static_pad("sink") {
                                                    // Check if this decodebin pad is already connected
                                                    if pad.peer().is_some() {
                                                        debug!("🔍 DEBUG: {} Pad {} already linked, skipping", pipeline_name_clone, pad.name());
                                                        break;
                                                    }
                                                    
                                                    // Check if audioconvert sink is already connected to a different pad
                                                    if let Some(current_peer) = sink_pad.peer() {
                                                        debug!("🔗 DEBUG: {} Unlinking audioconvert from existing pad: {}", pipeline_name_clone, current_peer.name());
                                                        if let Err(err) = current_peer.unlink(&sink_pad) {
                                                            debug!("⚠️ DEBUG: {} Unlink error (may be expected): {}", pipeline_name_clone, err);
                                                        }
                                                    }
                                                    
                                                    // Link decodebin src pad to audioconvert sink
                                                    match pad.link(&sink_pad) {
                                                        Ok(_) => {
                                                            debug!("✅ DEBUG: {} Successfully linked {} -> audioconvert.sink", pipeline_name_clone, pad.name());
                                                        }
                                                        Err(err) => {
                                                            warn!("❌ DEBUG: {} Failed to link {} -> audioconvert.sink: {}", pipeline_name_clone, pad.name(), err);
                                                        }
                                                    }
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                }
                            })
                        );
                        info!("✅ Added decodebin pad-added handler for pipeline: {}", pipeline_name);
                        break;
                    }
                } else {
                    debug!("ℹ️ Pipeline {} has no decodebin element", in_bin.name());
                }
            }
        }
    }

    let p = match decode_bin_sink_pad {
        Some(pad) => pad,
        None => {
            return Err(format!(
                "could not find decode_bin sink element for bin: {}",
                in_bin.name()
            ));
        }
    };

    debug!("🔗 {}: Attaching pad-added signal closure on sdp-demux...", in_bin.name());
    sdpdemux.connect_closure(
        "pad-added",
        false,
        glib::closure!(|bin: &gst::Element, pad: &gst::Pad| {
            debug!("🔗 Pad {} added in {}", pad.name(), bin.name());

            //when stream_0 pad is added.. (this happens every time a new ssrc tries to connect, it creates stream_0)
            if pad.name() == "stream_0" {
                info!("🔄 {}: sdpdemux stream_0 pad added, (re)-link after potential stream switch...", bin.name());

                let p = p.to_owned();
                pad.add_probe(gst::PadProbeType::IDLE, move |pad, _probe_info| {
                    // Unlink any existing connection first
                    if let Some(peer) = p.peer() {
                        let _ = peer.unlink(&p);
                    }
                    // Link to the new pad
                    match pad.link(&p) {
                        Ok(_) => info!("✅ Successfully linked new stream_0 to decodebin"),
                        Err(e) => warn!("❌ Failed to link stream_0 to decodebin: {:?}", e),
                    }
                    gst::PadProbeReturn::Remove
                });
            }
        }),
    );

    Ok(())    
}
 
fn get_available_interfaces() -> Vec<String> {
    let mut interfaces = Vec::new();
    
    // Try to read /sys/class/net/ directory to get available interfaces
    if let Ok(entries) = fs::read_dir("/sys/class/net") {
        for entry in entries.filter_map(Result::ok) {
            if let Some(name) = entry.file_name().to_str() {
                // Skip loopback interface
                if name != "lo" {
                    interfaces.push(name.to_string());
                }
            }
        }
    }
    
    // If no interfaces detected, we'll handle this in the selection function
    if interfaces.is_empty() {
        warn!("⚠️ Could not detect interfaces automatically from /sys/class/net/");
    }
    
    // Sort interfaces for consistent display
    interfaces.sort();
    interfaces
}

fn validate_interface(interface: &str) -> bool {
    // Check if interface exists in /sys/class/net/
    let path = format!("/sys/class/net/{}", interface);
    std::path::Path::new(&path).exists()
}



fn show_interface_info(interface: &str) {
    let path = format!("/sys/class/net/{}", interface);
    
    if std::path::Path::new(&path).exists() {
        // Try to read interface state
        let state_path = format!("/sys/class/net/{}/operstate", interface);
        if let Ok(state) = fs::read_to_string(&state_path) {
            let state = state.trim();
            match state {
                "up" => info!("✅ Interface {} is UP and ready", interface),
                "down" => warn!("⚠️ Interface {} is DOWN", interface),
                "unknown" => warn!("❓ Interface {} state is UNKNOWN", interface),
                _ => info!("ℹ️ Interface {} state: {}", interface, state),
            }
        }
        
        // Try to read MAC address
        let addr_path = format!("/sys/class/net/{}/address", interface);
        if let Ok(addr) = fs::read_to_string(&addr_path) {
            debug!("📡 MAC Address: {}", addr.trim());
        }
    } else {
        warn!("⚠️ Interface {} not found in system", interface);
    }
}

fn get_network_interface() -> Result<String, Error> {
    println!("🌐 Network Interface Selection");
    
    let interfaces = get_available_interfaces();
    
    if interfaces.is_empty() {
        println!("❓ No network interfaces detected automatically.");
        println!("Please enter the interface name manually:");
        
        loop {
            print!("Interface name: ");
            io::stdout().flush().unwrap();
            
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let interface = input.trim().to_string();
            
            if !interface.is_empty() {
                if validate_interface(&interface) {
                    show_interface_info(&interface);
                    return Ok(interface);
                } else {
                    println!("⚠️ Interface '{}' not found on this system.", interface);
                    print!("Continue anyway? (y/N): ");
                    io::stdout().flush().unwrap();
                    let mut confirm = String::new();
                    io::stdin().read_line(&mut confirm)?;
                    if confirm.trim().to_lowercase() == "y" {
                        return Ok(interface);
                    }
                    println!("Please try again with a valid interface name.");
                }
            } else {
                println!("❌ Interface name cannot be empty. Please try again.");
            }
        }
    } else {
        println!("Available network interfaces on this system:");
        for (i, interface) in interfaces.iter().enumerate() {
            if i == 0 {
                println!("  {}. {} (default)", i + 1, interface);
            } else {
                println!("  {}. {}", i + 1, interface);
            }
        }
        println!("  {}. Custom interface", interfaces.len() + 1);
        
        loop {
            print!("\nSelect interface (1-{}) or press Enter for default ({}): ", 
                   interfaces.len() + 1, 
                   interfaces.first().unwrap_or(&"eth0".to_string()));
            io::stdout().flush().unwrap();
            
            let mut input = String::new();
            io::stdin().read_line(&mut input)?;
            let choice = input.trim();
            
            if choice.is_empty() {
                // Return first interface as default
                let default_interface = interfaces.first().unwrap_or(&"eth0".to_string()).clone();
                show_interface_info(&default_interface);
                return Ok(default_interface);
            }
            
            if let Ok(num) = choice.parse::<usize>() {
                if num >= 1 && num <= interfaces.len() {
                    let selected_interface = &interfaces[num - 1];
                    show_interface_info(selected_interface);
                    return Ok(selected_interface.clone());
                } else if num == interfaces.len() + 1 {
                // Custom interface option
                print!("Enter custom interface name: ");
                io::stdout().flush().unwrap();
                let mut custom = String::new();
                io::stdin().read_line(&mut custom)?;
                let interface = custom.trim().to_string();
                if !interface.is_empty() {
                    if validate_interface(&interface) {
                        show_interface_info(&interface);
                        return Ok(interface);
                    } else {
                        println!("⚠️ Interface '{}' not found on this system.", interface);
                        print!("Continue anyway? (y/N): ");
                        io::stdout().flush().unwrap();
                        let mut confirm = String::new();
                        io::stdin().read_line(&mut confirm)?;
                        if confirm.trim().to_lowercase() == "y" {
                            return Ok(interface);
                        }
                        println!("Please try again with a valid interface name.");
                    }
                } else {
                    println!("❌ Interface name cannot be empty. Please try again.");
                }
            } else {
                println!("❌ Invalid selection. Please choose 1-{} or press Enter for default.", interfaces.len() + 1);
            }
        } else {
            println!("❌ Please enter a number or press Enter for default.");
        }
        }
    }
}

fn main() -> Result<(), Error> {
    // Initialize tracing subscriber with environment-based filtering
    // Set log level with RUST_LOG env var (e.g., RUST_LOG=debug cargo run)
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gstreamer_pipeline_manager=info".into())
        )
        .init();
    
    gstreamer::init()?;
    
    // Get network interface selection
    let network_interface = get_network_interface()?;
    info!("✅ Selected network interface: {}", network_interface);

    let mut manager = PipelineManager::new(network_interface.clone());

    // Create pipelines with selected network interface
    let p1_desc = format!("audiotestsrc samplesperbuffer=48 is-live=true ! audio/x-raw,rate=48000,channels=2 ! audioconvert ! rtpL24pay pt=98 timestamp-offset=0 max-ptime=1000000 min-ptime=1000000 ! udpsink processing-deadline=1000000 multicast-iface=\"{}\" ttl-mc=64 bind-port=5004 host=\"239.0.0.10\"", network_interface);
    let p2_desc = format!("filesrc location=./from_pipeline1.sdp ! sdpdemux latency=6 timeout=0 ! decodebin ! audioconvert ! rtpL24pay pt=98 timestamp-offset=0 max-ptime=1000000 min-ptime=1000000 ! udpsink processing-deadline=1000000 multicast-iface=\"{}\" ttl-mc=64 bind-port=5004 host=\"239.0.0.11\"", network_interface);
    let p3_desc = format!("filesrc location=./from_pipeline2.sdp ! sdpdemux latency=6 timeout=0 ! decodebin ! audioconvert ! rtpL24pay pt=98 timestamp-offset=0 max-ptime=1000000 min-ptime=1000000 ! udpsink processing-deadline=1000000 multicast-iface=\"{}\" ttl-mc=64 bind-port=5004 host=\"239.0.0.12\"", network_interface);
    
    manager.create_pipeline("p1", &p1_desc)?;
    manager.create_pipeline("p2", &p2_desc)?;
    manager.create_pipeline("p3", &p3_desc)?;

    // Initialize rustyline editor for command history and line editing
    let mut rl = DefaultEditor::new()?;
    
    // Show help once at startup
    println!("\n📋 Available Commands:");
    println!("  start <name>           - Start a pipeline");
    println!("  stop <name>            - Stop a pipeline");
    println!("  remove <name>          - Remove a pipeline");
    println!("  toggle <name>          - Toggle pipeline (stop->wait->start)");
    println!("  toggle <name> <secs>   - Toggle with custom timeout");
    println!("  list                   - List all pipelines with latency");
    println!("  latency                - Show detailed latency information");
    println!("  help                   - Show this help");
    println!("  exit                   - Exit the program");
    println!("\n💡 Use ↑/↓ arrow keys to navigate command history");

    loop {
        let readline = rl.readline("RTP> ");
        match readline {
            Ok(line) => {
                let trimmed = line.trim();
                if !trimmed.is_empty() {
                    rl.add_history_entry(trimmed)?;
                    
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    
                    match parts.as_slice() {
                        ["start", name] => manager.start_pipeline(name)?,
                        ["stop", name] => manager.stop_pipeline(name)?,
                        ["remove", name] => manager.remove_pipeline(name)?,
                        ["toggle", name] => {
                            // Default timeout of 2 seconds
                            manager.toggle_pipeline(name, 2)?;
                        },
                        ["toggle", name, timeout_str] => {
                            match timeout_str.parse::<u64>() {
                                Ok(timeout) => manager.toggle_pipeline(name, timeout)?,
                                Err(_) => println!("❌ Invalid timeout value. Please enter a number."),
                            }
                        },
                        ["list"] => manager.list_pipelines(),
                        ["latency"] => manager.show_latency_info(),
                        ["help"] => {
                            println!("\n📋 Available Commands:");
                            println!("  start <name>           - Start a pipeline");
                            println!("  stop <name>            - Stop a pipeline");
                            println!("  remove <name>          - Remove a pipeline");
                            println!("  toggle <name>          - Toggle pipeline (stop->wait->start)");
                            println!("  toggle <name> <secs>   - Toggle with custom timeout");
                            println!("  list                   - List all pipelines with latency");
                            println!("  latency                - Show detailed latency information");
                            println!("  help                   - Show this help");
                            println!("  exit                   - Exit the program");
                        },
                        ["exit"] | ["quit"] => break,
                        [] => {}, // Empty command, do nothing
                        _ => println!("❓ Unknown command '{}'. Type 'help' for available commands.", trimmed),
                    }
                }
            },
            Err(ReadlineError::Interrupted) => {
                println!("🛑 Ctrl-C pressed. Type 'exit' to quit.");
            },
            Err(ReadlineError::Eof) => {
                println!("👋 Goodbye!");
                break;
            },
            Err(err) => {
                println!("❌ Error reading input: {:?}", err);
                break;
            }
        }
    }

    Ok(())
}