use mel_spec::prelude::*;
use mel_spec::vad::{duration_ms_for_n_frames, format_milliseconds};
use mel_spec_audio::deinterleave_vecs_f32;
use mel_spec_pipeline::{Pipeline, PipelineConfig};
use std::io::{self, Read};
use std::thread;
use structopt::StructOpt;
use whisper_rs::*;

#[derive(Debug, StructOpt)]
#[structopt(name = "mel_spec_example", about = "mel_spec whisper example app")]
struct Command {
    #[structopt(
        short,
        long,
        default_value = "./../../../whisper.cpp/models/ggml-medium.en.bin"
    )]
    model_path: String,
    #[structopt(short, long, default_value = "./mel_out")]
    out_path: String,
    #[structopt(long, default_value = "1.0")]
    energy_threshold: f64,
    #[structopt(long, default_value = "5")]
    min_intersections: usize,
    #[structopt(long, default_value = "10")]
    intersection_threshold: usize,
    #[structopt(long, default_value = "10")]
    min_mel: usize,
    #[structopt(long, default_value = "100")]
    min_frames: usize,
}

fn main() {
    let args = Command::from_args();
    let model_path = args.model_path;
    let mel_path = args.out_path;

    let energy_threshold = args.energy_threshold;
    let min_intersections = args.min_intersections;
    let intersection_threshold = args.intersection_threshold;
    let min_mel = args.min_mel;
    let min_frames = args.min_frames;

    let fft_size = 400;
    let hop_size = 160;
    let n_mels = 80;
    let sampling_rate = 16000.0;

    let mel_settings = MelConfig::new(fft_size, hop_size, n_mels, sampling_rate);
    let vad_settings = DetectionSettings::new(
        energy_threshold,
        min_intersections,
        intersection_threshold,
        min_mel,
        min_frames,
    );

    let config = PipelineConfig::new(mel_settings, vad_settings);

    let mut pipeline = Pipeline::new(config);

    let rx_clone = pipeline.rx();
    let mut handles = pipeline.start();

    let handle = thread::spawn(move || {
        let ctx = WhisperContext::new(&model_path).expect("failed to load model");
        let mut state = ctx.create_state().expect("failed to create key");

        while let Ok((idx, mel)) = rx_clone.recv() {
            let path = format!("{}/frame_{}.tga", mel_path, idx);
            let _ = save_tga_8bit(&mel, n_mels, &path);

            let ms = duration_ms_for_n_frames(hop_size, sampling_rate, idx);
            let time = format_milliseconds(ms as u64);

            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 0 });
            params.set_n_threads(1);
            params.set_single_segment(true);
            params.set_language(Some("en"));
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            state.set_mel(&mel).unwrap();
            let empty = vec![];
            state.full(params, &empty[..]).unwrap();

            let num_segments = state.full_n_segments().unwrap();
            if num_segments > 0 {
                if let Ok(text) = state.full_get_segment_text(0) {
                    let msg = format!("{} [{}] {}", idx, time, text);
                    println!("{}", msg);
                } else {
                    println!("Error retrieving text for segment.");
                }
            }
        }
    });

    handles.push(handle);

    // read audio from pipe
    const LEN: usize = 128;
    let mut input: Box<dyn Read> = Box::new(io::stdin());

    let mut buffer = [0; LEN];
    let mut bytes_read = 0;
    loop {
        match input.read(&mut buffer[bytes_read..]) {
            Ok(0) => break,
            Ok(n) => bytes_read += n,
            Err(error) => {
                eprintln!("Error reading input: {}", error);
                std::process::exit(1);
            }
        }

        if bytes_read == LEN {
            let samples = deinterleave_vecs_f32(&buffer, 1);
            for chunk in samples[0].chunks(LEN / 4) {
                let _ = pipeline.send_pcm(chunk);
            }
            bytes_read = 0;
        }
    }

    if bytes_read > 0 {
        let samples = deinterleave_vecs_f32(&buffer[..bytes_read], 1);
        for chunk in samples[0].chunks(bytes_read / 4) {
            let _ = pipeline.send_pcm(chunk);
        }
    }

    pipeline.close_ingress();

    for handle in handles {
        handle.join().unwrap();
    }
}