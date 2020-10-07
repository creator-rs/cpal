extern crate core_foundation_sys;
extern crate coreaudio;

use std::cell::RefCell;
use std::ops::DerefMut;
use std::ptr::null_mut;
use std::sync::{Arc, Mutex, RwLock};

use traits::{DeviceTrait, HostTrait, StreamTrait};

use crate::{
    BackendSpecificError, BufferSize, BuildStreamError, Data, DefaultStreamConfigError,
    DeviceNameError, DevicesError, InputCallbackInfo, OutputCallbackInfo, PauseStreamError,
    PlayStreamError, SampleFormat, SampleRate, StreamConfig, StreamError, SupportedBufferSize,
    SupportedStreamConfig, SupportedStreamConfigRange, SupportedStreamConfigsError,
};

use self::coreaudio::audio_unit::{AudioUnit, Element, render_callback, Scope};
use self::coreaudio::audio_unit::render_callback::data;
use self::coreaudio::sys::{
    AudioBuffer,
    AudioComponent,
    AudioComponentDescription,
    AudioComponentFindNext,
    AudioComponentInstance,
    AudioComponentInstanceNew,
    AudioStreamBasicDescription,
    AudioUnitInitialize,
    AudioValueRange,
    kAudioFormatFlagIsFloat,
    kAudioFormatFlagIsPacked,
    kAudioFormatLinearPCM,
    kAudioUnitManufacturer_Apple,
    kAudioUnitProperty_StreamFormat,
    kAudioUnitSubType_RemoteIO,
    kAudioUnitType_Output,
    OSStatus,

};

pub struct Devices;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device;

pub struct Host;

pub type SupportedInputConfigs = ::std::vec::IntoIter<SupportedStreamConfigRange>;
pub type SupportedOutputConfigs = ::std::vec::IntoIter<SupportedStreamConfigRange>;

const MIN_CHANNELS: u16 = 1;
const MAX_CHANNELS: u16 = 2;
const MIN_SAMPLE_RATE: SampleRate = SampleRate(44_100);
const MAX_SAMPLE_RATE: SampleRate = SampleRate(44_100);
const DEFAULT_SAMPLE_RATE: SampleRate = SampleRate(44_100);
const MIN_BUFFER_SIZE: u32 = 512;
const MAX_BUFFER_SIZE: u32 = 512;
const DEFAULT_BUFFER_SIZE: usize = 512;
const SUPPORTED_SAMPLE_FORMAT: SampleFormat = SampleFormat::F32;

impl Host {
    pub fn new() -> Result<Self, crate::HostUnavailable> {
        Ok(Host)
    }
}

impl HostTrait for Host {
    type Devices = Devices;
    type Device = Device;

    fn is_available() -> bool {
        true
    }

    fn devices(&self) -> Result<Self::Devices, DevicesError> {
        Devices::new()
    }

    fn default_input_device(&self) -> Option<Self::Device> {
        default_input_device()
    }

    fn default_output_device(&self) -> Option<Self::Device> {
        default_output_device()
    }
}

impl Devices {
    fn new() -> Result<Self, DevicesError> {
        Ok(Self::default())
    }
}

impl Device {
    #[inline]
    fn name(&self) -> Result<String, DeviceNameError> {
        Ok("Default Device".to_owned())
    }

    #[inline]
    fn supported_input_configs(
        &self,
    ) -> Result<SupportedInputConfigs, SupportedStreamConfigsError> {
        // TODO
        Ok(Vec::new().into_iter())
    }

    #[inline]
    fn supported_output_configs(
        &self,
    ) -> Result<SupportedOutputConfigs, SupportedStreamConfigsError> {
        let buffer_size = SupportedBufferSize::Range {
            min: MIN_BUFFER_SIZE,
            max: MAX_BUFFER_SIZE,
        };
        let configs: Vec<_> = (MIN_CHANNELS..=MAX_CHANNELS)
            .map(|channels| SupportedStreamConfigRange {
                channels,
                min_sample_rate: MIN_SAMPLE_RATE,
                max_sample_rate: MAX_SAMPLE_RATE,
                buffer_size: buffer_size.clone(),
                sample_format: SUPPORTED_SAMPLE_FORMAT,
            })
            .collect();
        Ok(configs.into_iter())
    }

    #[inline]
    fn default_input_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        // TODO
        Err(DefaultStreamConfigError::StreamTypeNotSupported)
    }

    #[inline]
    fn default_output_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        const EXPECT: &str = "expected at least one valid coreaudio stream config";
        let config = self
            .supported_output_configs()
            .expect(EXPECT)
            .max_by(|a, b| a.cmp_default_heuristics(b))
            .unwrap()
            .with_sample_rate(DEFAULT_SAMPLE_RATE);

        Ok(config)
    }
}

impl DeviceTrait for Device {
    type SupportedInputConfigs = SupportedInputConfigs;
    type SupportedOutputConfigs = SupportedOutputConfigs;
    type Stream = Stream;

    #[inline]
    fn name(&self) -> Result<String, DeviceNameError> {
        Device::name(self)
    }

    #[inline]
    fn supported_input_configs(
        &self,
    ) -> Result<Self::SupportedInputConfigs, SupportedStreamConfigsError> {
        Device::supported_input_configs(self)
    }

    #[inline]
    fn supported_output_configs(
        &self,
    ) -> Result<Self::SupportedOutputConfigs, SupportedStreamConfigsError> {
        Device::supported_output_configs(self)
    }

    #[inline]
    fn default_input_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        Device::default_input_config(self)
    }

    #[inline]
    fn default_output_config(&self) -> Result<SupportedStreamConfig, DefaultStreamConfigError> {
        Device::default_output_config(self)
    }

    fn build_input_stream_raw<D, E>(
        &self,
        _config: &StreamConfig,
        _sample_format: SampleFormat,
        _data_callback: D,
        _error_callback: E,
    ) -> Result<Self::Stream, BuildStreamError>
        where
            D: FnMut(&Data, &InputCallbackInfo) + Send + 'static,
            E: FnMut(StreamError) + Send + 'static,
    {
        // TODO
        Err(BuildStreamError::StreamConfigNotSupported)
    }

    /// Create an output stream.
    fn build_output_stream_raw<D, E>(
        &self,
        config: &StreamConfig,
        sample_format: SampleFormat,
        mut data_callback: D,
        mut error_callback: E,
    ) -> Result<Self::Stream, BuildStreamError>
        where
            D: FnMut(&mut Data, &OutputCallbackInfo) + Send + 'static,
            E: FnMut(StreamError) + Send + 'static,
    {
        println!("build output stream raw");
        if !valid_config(config, sample_format) {
            return Err(BuildStreamError::StreamConfigNotSupported);
        }

        let n_channels = config.channels as usize;

        let buffer_size_frames = match config.buffer_size {
            BufferSize::Fixed(v) => {
                if v == 0 {
                    return Err(BuildStreamError::StreamConfigNotSupported);
                } else {
                    v as usize
                }
            }
            BufferSize::Default => DEFAULT_BUFFER_SIZE,
        };
        // let buffer_size_samples = buffer_size_frames * n_channels;
        // let buffer_time_step_secs = buffer_time_step_secs(buffer_size_frames, config.sample_rate);

        // let data_callback = Arc::new(Mutex::new(Box::new(data_callback)));

        // let mut audio_unit = audio_unit_from_device(self, false)?;

        // let desc = AudioComponentDescription {
        //     componentType: kAudioUnitType_Output,
        //     componentSubType: kAudioUnitSubType_RemoteIO,
        //     componentManufacturer: kAudioUnitManufacturer_Apple,
        //     componentFlags: 0,
        //     componentFlagsMask: 0,
        // };
        //
        // //  Next, we get the first (and only) component corresponding to that description
        // let output_component: AudioComponent = AudioComponentFindNext(null_mut(), &desc);

        let au_type = coreaudio::audio_unit::IOType::RemoteIO;
        println!("new audio unit");
        let mut audio_unit = AudioUnit::new(au_type)?;

        // The scope and element for working with a device's output stream.
        let scope = Scope::Input;
        let element = Element::Output;

        println!("asbd");
        // Set the stream in interleaved mode.
        let asbd = asbd_from_config(config, sample_format);
        audio_unit.set_property(kAudioUnitProperty_StreamFormat, scope, element, Some(&asbd))?;

        // Set the buffersize
        // match config.buffer_size {
        //     BufferSize::Fixed(v) => {
        //         let buffer_size_range = get_io_buffer_frame_size_range(&audio_unit)?;
        //         match buffer_size_range {
        //             SupportedBufferSize::Range { min, max } => {
        //                 if v >= min && v <= max {
        //                     audio_unit.set_property(
        //                         kAudioDevicePropertyBufferFrameSize,
        //                         scope,
        //                         element,
        //                         Some(&v),
        //                     )?
        //                 } else {
        //                     return Err(BuildStreamError::StreamConfigNotSupported);
        //                 }
        //             }
        //             SupportedBufferSize::Unknown => (),
        //         }
        //     }
        //     BufferSize::Default => (),
        // }

        println!("register callback");
        // Register the callback that is being called by coreaudio whenever it needs data to be
        // fed to the audio buffer.
        let bytes_per_channel = sample_format.sample_size();
        let sample_rate = config.sample_rate;
        type Args = render_callback::Args<data::Raw>;
        audio_unit.set_render_callback(move |args: Args| unsafe {
            // If `run()` is currently running, then a callback will be available from this list.
            // Otherwise, we just fill the buffer with zeroes and return.
            // println!("cb");

            let AudioBuffer {
                mNumberChannels: channels,
                mDataByteSize: data_byte_size,
                mData: data,
            } = (*args.data.data).mBuffers[0];

            let data = data as *mut ();
            let len = (data_byte_size as usize / bytes_per_channel) as usize;
            let mut data = Data::from_parts(data, len, sample_format);

            let callback = match host_time_to_stream_instant(args.time_stamp.mHostTime) {
                Err(err) => {
                    println!("doh err");
                    error_callback(err.into());
                    return Err(());
                }
                Ok(cb) => cb,
            };
            // TODO: Need a better way to get delay, for now we assume a double-buffer offset.
            let buffer_frames = len / channels as usize;
            let delay = frames_to_duration(buffer_frames, sample_rate);
            let playback = callback
                .add(delay)
                .expect("`playback` occurs beyond representation supported by `StreamInstant`");
            let timestamp = crate::OutputStreamTimestamp { callback, playback };

            let info = OutputCallbackInfo { timestamp };
            data_callback(&mut data, &info);
            Ok(())
        })?;

        println!("start");
        audio_unit.start()?;
        println!("returning");

        Ok(Stream::new(StreamInner {
            playing: true,
            audio_unit,
        }))
    }
}

pub struct Stream {
    inner: RefCell<StreamInner>,
}

impl Stream {
    fn new(inner: StreamInner) -> Self {
        Self {
            inner: RefCell::new(inner),
        }
    }
}

impl StreamTrait for Stream {
    fn play(&self) -> Result<(), PlayStreamError> {
        let mut stream = self.inner.borrow_mut();

        if !stream.playing {
            if let Err(e) = stream.audio_unit.start() {
                let description = format!("{}", e);
                let err = BackendSpecificError { description };
                return Err(err.into());
            }
            stream.playing = true;
        }
        Ok(())
    }

    fn pause(&self) -> Result<(), PauseStreamError> {
        let mut stream = self.inner.borrow_mut();

        if stream.playing {
            if let Err(e) = stream.audio_unit.stop() {
                let description = format!("{}", e);
                let err = BackendSpecificError { description };
                return Err(err.into());
            }

            stream.playing = false;
        }
        Ok(())
    }
}

struct StreamInner {
    playing: bool,
    audio_unit: AudioUnit,
}

// TODO need stronger error identification
impl From<coreaudio::Error> for BuildStreamError {
    fn from(err: coreaudio::Error) -> BuildStreamError {
        match err {
            coreaudio::Error::RenderCallbackBufferFormatDoesNotMatchAudioUnitStreamFormat
            | coreaudio::Error::NoKnownSubtype
            | coreaudio::Error::AudioUnit(coreaudio::error::AudioUnitError::FormatNotSupported)
            | coreaudio::Error::AudioCodec(_)
            | coreaudio::Error::AudioFormat(_) => BuildStreamError::StreamConfigNotSupported,
            _ => BuildStreamError::DeviceNotAvailable,
        }
    }
}

impl From<coreaudio::Error> for SupportedStreamConfigsError {
    fn from(err: coreaudio::Error) -> SupportedStreamConfigsError {
        let description = format!("{}", err);
        let err = BackendSpecificError { description };
        // Check for possible DeviceNotAvailable variant
        SupportedStreamConfigsError::BackendSpecific { err }
    }
}

impl From<coreaudio::Error> for DefaultStreamConfigError {
    fn from(err: coreaudio::Error) -> DefaultStreamConfigError {
        let description = format!("{}", err);
        let err = BackendSpecificError { description };
        // Check for possible DeviceNotAvailable variant
        DefaultStreamConfigError::BackendSpecific { err }
    }
}

impl Default for Devices {
    fn default() -> Devices {
        Devices
    }
}

impl Iterator for Devices {
    type Item = Device;
    #[inline]
    fn next(&mut self) -> Option<Device> {
        Some(d)
    }
}

#[inline]
fn default_input_device() -> Option<Device> {
    // TODO
    None
}

#[inline]
fn default_output_device() -> Option<Device> {
    Some(Device)
}

// Whether or not the given stream configuration is valid for building a stream.
fn valid_config(conf: &StreamConfig, sample_format: SampleFormat) -> bool {
    conf.channels <= MAX_CHANNELS
        && conf.channels >= MIN_CHANNELS
        && conf.sample_rate <= MAX_SAMPLE_RATE
        && conf.sample_rate >= MIN_SAMPLE_RATE
        && sample_format == SUPPORTED_SAMPLE_FORMAT
}

fn buffer_time_step_secs(buffer_size_frames: usize, sample_rate: SampleRate) -> f64 {
    buffer_size_frames as f64 / sample_rate.0 as f64
}
