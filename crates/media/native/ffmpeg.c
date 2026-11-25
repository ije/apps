#include <libavcodec/avcodec.h>
#include <libavformat/avformat.h>
#include <libavutil/hwcontext.h>
#include <libavutil/pixdesc.h>
#include <libavutil/time.h>
#include <libswresample/swresample.h>
#include <libswscale/swscale.h>
#include <math.h>
#include <string.h>

#ifdef __linux__
#include <dlfcn.h>
#include <libavutil/hwcontext_vaapi.h>
#include <unistd.h>
#include <va/va_drmcommon.h>
#endif

// The decoder has this many frames more than it needs, which surfaces can hold.
#define SURFACES 6

typedef struct {
  double duration;
  int width, height, audio;
} AppsMediaInfo;

typedef struct {
  AVCodecContext *codec;
  AVStream *stream;
  AVPacket **queue;
  int64_t bytes;
  int head, length, capacity, sent_end, ended;
} Track;

typedef struct {
  AVFormatContext *format;
  AVIOContext *io;
  const uint8_t *data;
  int64_t size, position;
  Track video, audio;
  AVBufferRef *device;
  enum AVPixelFormat hardware;
  AVFrame *frame, *software, *picture;
  AVPacket *packet;
  struct SwsContext *scale;
  SwrContext *resample;
  AVChannelLayout layout, input;
  int rate, input_format, input_rate, width, height, read_end, software_only, slow;
  // Frames and audio packets up to these times came before the video changed to software.
  // Each surface holds a reference.
  AVBufferRef *surfaces;
  int64_t video_resume, audio_resume, audio_read;
  double speed;
  float *samples;
  int capacity;
} AppsMedia;

static int memory_read(void *opaque, uint8_t *buffer, int size) {
  AppsMedia *m = opaque;
  int64_t left = m->size - m->position;
  if (left <= 0) return AVERROR_EOF;
  if (size > left) size = (int)left;
  memcpy(buffer, m->data + m->position, size);
  m->position += size;
  return size;
}

static int64_t memory_seek(void *opaque, int64_t offset, int whence) {
  AppsMedia *m = opaque;
  if (whence & AVSEEK_SIZE) return m->size;
  whence &= ~AVSEEK_FORCE;
  int64_t base = whence == SEEK_CUR ? m->position : whence == SEEK_END ? m->size : 0;
  if (whence != SEEK_SET && whence != SEEK_CUR && whence != SEEK_END) return AVERROR(EINVAL);
  if (base + offset < 0 || base + offset > m->size) return AVERROR(EINVAL);
  return m->position = base + offset;
}

static enum AVPixelFormat pick_format(AVCodecContext *codec, const enum AVPixelFormat *formats) {
  AppsMedia *m = codec->opaque;
  for (const enum AVPixelFormat *format = formats; *format != AV_PIX_FMT_NONE; format++)
    if (*format == m->hardware) return *format;
  for (const enum AVPixelFormat *format = formats; *format != AV_PIX_FMT_NONE; format++)
    if (!(av_pix_fmt_desc_get(*format)->flags & AV_PIX_FMT_FLAG_HWACCEL)) return *format;
  return AV_PIX_FMT_NONE;
}

static int open_track(AppsMedia *m, Track *track, enum AVMediaType type) {
  const AVCodec *decoder = NULL;
  int index = av_find_best_stream(m->format, type, -1, -1, &decoder, 0);
  if (index < 0 || !decoder) return 0;
  AVStream *stream = m->format->streams[index];
  if (stream->disposition & AV_DISPOSITION_ATTACHED_PIC) return 0;
  AVCodecContext *codec = avcodec_alloc_context3(decoder);
  if (!codec || avcodec_parameters_to_context(codec, stream->codecpar) < 0) {
    avcodec_free_context(&codec);
    return 0;
  }
  codec->pkt_timebase = stream->time_base;
  codec->thread_count = 0;
  codec->opaque = m;
  if (type == AVMEDIA_TYPE_VIDEO && !m->software_only) {
#if defined(__APPLE__)
    enum AVHWDeviceType device = AV_HWDEVICE_TYPE_VIDEOTOOLBOX;
#elif defined(_WIN32)
    enum AVHWDeviceType device = AV_HWDEVICE_TYPE_D3D11VA;
#else
    enum AVHWDeviceType device = AV_HWDEVICE_TYPE_VAAPI;
#endif
    for (int i = 0;; i++) {
      const AVCodecHWConfig *config = avcodec_get_hw_config(decoder, i);
      if (!config) break;
      if (config->device_type == device && config->methods & AV_CODEC_HW_CONFIG_METHOD_HW_DEVICE_CTX &&
          av_hwdevice_ctx_create(&m->device, device, NULL, NULL, 0) >= 0) {
        m->hardware = config->pix_fmt;
        codec->hw_device_ctx = av_buffer_ref(m->device);
        codec->get_format = pick_format;
        if (device == AV_HWDEVICE_TYPE_VAAPI) codec->extra_hw_frames = SURFACES;
        break;
      }
    }
  }
  if (avcodec_open2(codec, decoder, NULL) < 0) {
    avcodec_free_context(&codec);
    return 0;
  }
  track->codec = codec;
  track->stream = stream;
  return 1;
}

void apps_ffmpeg_close(AppsMedia *m);

AppsMedia *apps_ffmpeg_open(const char *path, const uint8_t *data, int64_t size, int video, int rate,
                           int channels, AppsMediaInfo *info, char *error, int error_size) {
  av_log_set_level(AV_LOG_QUIET);
  AppsMedia *m = av_mallocz(sizeof(AppsMedia));
  int status = AVERROR(ENOMEM);
  if (!m) goto fail;
  m->speed = 1;
  m->video_resume = m->audio_resume = m->audio_read = INT64_MIN;
  m->rate = rate;
  av_channel_layout_default(&m->layout, channels > 0 ? channels : 2);
  m->surfaces = av_buffer_allocz(1);
  m->format = avformat_alloc_context();
  m->frame = av_frame_alloc();
  m->software = av_frame_alloc();
  m->picture = av_frame_alloc();
  m->packet = av_packet_alloc();
  if (!m->surfaces || !m->format || !m->frame || !m->software || !m->picture || !m->packet) goto fail;
  if (data) {
    m->data = data;
    m->size = size;
    uint8_t *buffer = av_malloc(1 << 16);
    m->io = buffer ? avio_alloc_context(buffer, 1 << 16, 0, m, memory_read, NULL, memory_seek) : NULL;
    if (!m->io) {
      av_free(buffer);
      goto fail;
    }
    m->format->pb = m->io;
  }
  if ((status = avformat_open_input(&m->format, data ? NULL : path, NULL, NULL)) < 0) goto fail;
  if ((status = avformat_find_stream_info(m->format, NULL)) < 0) goto fail;
  if (video && open_track(m, &m->video, AVMEDIA_TYPE_VIDEO)) {
    AVRational aspect = av_guess_sample_aspect_ratio(m->format, m->video.stream, NULL);
    double width = m->video.stream->codecpar->width, height = m->video.stream->codecpar->height;
    if (aspect.num > 0 && aspect.den > 0) width *= av_q2d(aspect);
    // Cap frame dimensions at 8192 to bound allocations from media input.
    double fit = fmin(1, 8192 / fmax(width, height));
    if (width > 0 && height > 0) {
      m->width = (int)fmax(1, lrint(width * fit));
      m->height = (int)fmax(1, lrint(height * fit));
    } else {
      // Without a size the caller never wants video, so its packets must not queue.
      avcodec_free_context(&m->video.codec);
      m->video.stream = NULL;
    }
  }
  if (rate > 0) open_track(m, &m->audio, AVMEDIA_TYPE_AUDIO);
  status = AVERROR_DECODER_NOT_FOUND;
  if (!m->video.codec && !m->audio.codec) goto fail;
  for (unsigned i = 0; i < m->format->nb_streams; i++)
    if (m->format->streams[i] != m->video.stream && m->format->streams[i] != m->audio.stream)
      m->format->streams[i]->discard = AVDISCARD_ALL;
  info->duration = m->format->duration == AV_NOPTS_VALUE ? INFINITY : m->format->duration / (double)AV_TIME_BASE;
  info->width = m->width;
  info->height = m->height;
  info->audio = m->audio.codec != NULL;
  return m;
fail:
  av_strerror(status, error, error_size);
  if (m) apps_ffmpeg_close(m);
  return NULL;
}

static void clear(Track *track) {
  for (; track->length; track->length--) {
    av_packet_free(&track->queue[track->head]);
    track->head = (track->head + 1) % track->capacity;
  }
  track->head = 0;
  track->bytes = 0;
  track->sent_end = track->ended = 0;
  if (track->codec) avcodec_flush_buffers(track->codec);
}

static int push(Track *track, AVPacket *packet) {
  if (track->length == track->capacity) {
    int capacity = track->capacity ? track->capacity * 2 : 64;
    AVPacket **queue = av_malloc_array(capacity, sizeof(AVPacket *));
    if (!queue) return AVERROR(ENOMEM);
    for (int i = 0; i < track->length; i++) queue[i] = track->queue[(track->head + i) % track->capacity];
    av_free(track->queue);
    track->queue = queue;
    track->capacity = capacity;
    track->head = 0;
  }
  AVPacket *copy = av_packet_alloc();
  if (!copy) return AVERROR(ENOMEM);
  track->bytes += packet->size;
  av_packet_move_ref(copy, packet);
  track->queue[(track->head + track->length++) % track->capacity] = copy;
  return 0;
}

static int feed(AppsMedia *m, Track *track) {
  if (track->length) {
    AVPacket *packet = track->queue[track->head];
    int status = avcodec_send_packet(track->codec, packet);
    if (status == AVERROR(EAGAIN)) return 1;
    track->head = (track->head + 1) % track->capacity;
    track->length--;
    track->bytes -= packet->size;
    av_packet_free(&packet);
    return status < 0 && status != AVERROR_INVALIDDATA ? status : 1;
  }
  if (!m->read_end) return 0;
  if (track->sent_end) track->ended = 1;
  track->sent_end = 1;
  avcodec_send_packet(track->codec, NULL);
  return 1;
}

static double seconds(AppsMedia *m, Track *track, int64_t time) {
  if (time == AV_NOPTS_VALUE) return NAN;
  int64_t start = m->format->start_time == AV_NOPTS_VALUE ? 0 : m->format->start_time;
  return time * av_q2d(track->stream->time_base) - start / (double)AV_TIME_BASE;
}

static int audio(AppsMedia *m, const float **samples, int *count, double *time) {
  AVFrame *frame = m->frame;
  // Recreate the resampler on format changes: swr_convert reads planes using its original layout.
  if (m->resample && (frame->format != m->input_format || frame->sample_rate != m->input_rate ||
                      av_channel_layout_compare(&frame->ch_layout, &m->input)))
    swr_free(&m->resample);
  if (!m->resample) {
    int rate = (int)lrint(m->rate / m->speed);
    if (swr_alloc_set_opts2(&m->resample, &m->layout, AV_SAMPLE_FMT_FLT, rate, &frame->ch_layout,
                            frame->format, frame->sample_rate, 0, NULL) < 0 ||
        swr_init(m->resample) < 0 || av_channel_layout_copy(&m->input, &frame->ch_layout) < 0) {
      swr_free(&m->resample);
      return AVERROR(EINVAL);
    }
    m->input_format = frame->format;
    m->input_rate = frame->sample_rate;
  }
  int needed = swr_get_out_samples(m->resample, frame->nb_samples);
  if (needed > m->capacity) {
    av_free(m->samples);
    m->samples = av_malloc_array(needed, m->layout.nb_channels * sizeof(float));
    m->capacity = m->samples ? needed : 0;
    if (!m->samples) return AVERROR(ENOMEM);
  }
  uint8_t *out = (uint8_t *)m->samples;
  int converted = swr_convert(m->resample, &out, needed, (const uint8_t **)frame->extended_data, frame->nb_samples);
  if (converted < 0) {
    swr_free(&m->resample);
    return 3;
  }
  *samples = m->samples;
  *count = converted;
  *time = seconds(m, &m->audio, frame->best_effort_timestamp);
  return 2;
}

int apps_ffmpeg_next(AppsMedia *m, int want, double *time, int *width, int *height, const float **samples, int *count) {
  for (;;) {
    int waiting = 0;
    for (int kind = 1; kind <= 2; kind++) {
      Track *track = kind == 1 ? &m->video : &m->audio;
      if (!(want & kind) || !track->codec || track->ended) continue;
      int status = avcodec_receive_frame(track->codec, m->frame);
      if (status == AVERROR_EOF) {
        track->ended = 1;
        continue;
      }
      if (status == 0) {
        if (kind == 2) return audio(m, samples, count, time);
        if (m->frame->best_effort_timestamp != AV_NOPTS_VALUE && m->frame->best_effort_timestamp <= m->video_resume) {
          av_frame_unref(m->frame);
          waiting |= 1;
          continue;
        }
        *time = seconds(m, track, m->frame->best_effort_timestamp);
        av_frame_unref(m->picture);
        av_frame_move_ref(m->picture, m->frame);
        *width = m->width;
        *height = m->height;
        return 1;
      }
      if (status != AVERROR(EAGAIN)) return status;
      if ((status = feed(m, track)) < 0) return status;
      waiting |= status;
      if (!status) waiting |= 2;
    }
    if (!waiting) return 0;
    if (!(waiting & 2)) continue;
    int status = av_read_frame(m->format, m->packet);
    if (status == AVERROR_EOF || (status == AVERROR(EIO) && avio_feof(m->format->pb))) {
      m->read_end = 1;
      continue;
    }
    if (status < 0) return status;
    Track *track = m->video.stream && m->packet->stream_index == m->video.stream->index ? &m->video
                   : m->audio.stream && m->packet->stream_index == m->audio.stream->index ? &m->audio
                                                                                           : NULL;
    if (track == &m->audio && m->packet->dts != AV_NOPTS_VALUE) {
      if (m->packet->dts <= m->audio_resume) track = NULL;
      else m->audio_read = m->packet->dts;
    }
    if (track && (status = push(track, m->packet)) < 0) return status;
    av_packet_unref(m->packet);
    // Packets of a track that is not wanted stay in memory, so the caller decodes that track first.
    if (track && !(want & (track == &m->video ? 1 : 2)) && (track->length >= 4096 || track->bytes >= 32 << 20))
      return 4;
  }
}

static int color_matrix(const AVFrame *frame) {
  return frame->colorspace == AVCOL_SPC_BT709 ? SWS_CS_ITU709
         : frame->colorspace == AVCOL_SPC_BT2020_NCL || frame->colorspace == AVCOL_SPC_BT2020_CL ? SWS_CS_BT2020
         : frame->colorspace == AVCOL_SPC_UNSPECIFIED && frame->height >= 720                    ? SWS_CS_ITU709
                                                                                                  : SWS_CS_DEFAULT;
}

static int convert(struct SwsContext **scale, const AVFrame *frame, int width, int height, uint8_t *pixels) {
  *scale = sws_getCachedContext(*scale, frame->width, frame->height, frame->format, width, height, AV_PIX_FMT_BGRA,
                                SWS_BILINEAR, NULL, NULL, NULL);
  if (!*scale) return AVERROR(EINVAL);
  sws_setColorspaceDetails(*scale, sws_getCoefficients(color_matrix(frame)), frame->color_range == AVCOL_RANGE_JPEG,
                           sws_getCoefficients(SWS_CS_DEFAULT), 1, 0, 1 << 16, 1 << 16);
  uint8_t *planes[4] = {pixels};
  int strides[4] = {width * 4};
  int rows = sws_scale(*scale, (const uint8_t *const *)frame->data, frame->linesize, 0, frame->height, planes, strides);
  return rows == height ? 0 : AVERROR(EINVAL);
}

// A decoded frame in GPU memory. Change `Surface` in lib.rs and player.rs together with this.
typedef struct {
  void *handle;
  uint32_t width, height, matrix, full_range;
  int32_t fds[2];
  uint32_t offsets[2], pitches[2];
  uint64_t modifiers[2];
} AppsSurface;

typedef struct {
  AVFrame *frame;
  AVBufferRef *count;
  int fds[2];
} Held;

void apps_ffmpeg_release(void *handle) {
  Held *held = handle;
#ifdef __linux__
  for (int i = 0; i < 2; i++)
    if (held->fds[i] >= 0) close(held->fds[i]);
#endif
  av_frame_free(&held->frame);
  av_buffer_unref(&held->count);
  av_free(held);
}

int apps_ffmpeg_read(void *handle, int width, int height, uint8_t *pixels) {
  Held *held = handle;
  struct SwsContext *scale = NULL;
  AVFrame *frame = av_frame_alloc();
  int status = frame ? av_hwframe_transfer_data(frame, held->frame, 0) : AVERROR(ENOMEM);
  if (status >= 0) status = av_frame_copy_props(frame, held->frame);
  if (status >= 0) status = convert(&scale, frame, width, height, pixels);
  sws_freeContext(scale);
  av_frame_free(&frame);
  return status;
}

// Returns 0 when the GPU can't read the picture in place.
int apps_ffmpeg_surface(AppsMedia *m, AppsSurface *out) {
#ifdef __linux__
  AVFrame *frame = m->picture;
  if (frame->format != AV_PIX_FMT_VAAPI || !frame->hw_frames_ctx) return 0;
  AVHWFramesContext *frames = (AVHWFramesContext *)frame->hw_frames_ctx->data;
  if (frames->sw_format != AV_PIX_FMT_NV12) return 0;
  // FFmpeg has loaded libva.
  void *va = dlopen("libva.so.2", RTLD_NOW | RTLD_NOLOAD);
  if (!va) return 0;
  __typeof__(&vaSyncSurface) sync = (__typeof__(&vaSyncSurface))dlsym(va, "vaSyncSurface");
  __typeof__(&vaExportSurfaceHandle) export =
      (__typeof__(&vaExportSurfaceHandle))dlsym(va, "vaExportSurfaceHandle");
  dlclose(va);
  if (!sync || !export) return 0;
  if (av_buffer_get_ref_count(m->surfaces) > SURFACES) return 0;
  Held *held = av_mallocz(sizeof(Held));
  if (!held) return 0;
  *held = (Held){av_frame_clone(frame), av_buffer_ref(m->surfaces), {-1, -1}};
  if (!held->frame || !held->count) {
    apps_ffmpeg_release(held);
    return 0;
  }
  VADisplay display = ((AVVAAPIDeviceContext *)frames->device_ctx->hwctx)->display;
  VASurfaceID id = (VASurfaceID)(uintptr_t)frame->data[3];
  VADRMPRIMESurfaceDescriptor planes;
  if (sync(display, id) != VA_STATUS_SUCCESS ||
      export(display, id, VA_SURFACE_ATTRIB_MEM_TYPE_DRM_PRIME_2,
             VA_EXPORT_SURFACE_READ_ONLY | VA_EXPORT_SURFACE_SEPARATE_LAYERS, &planes) != VA_STATUS_SUCCESS) {
    apps_ffmpeg_release(held);
    return 0;
  }
  int usable = planes.num_layers == 2 && planes.layers[0].num_planes == 1 && planes.layers[1].num_planes == 1;
  *out = (AppsSurface){held, frame->width, frame->height, color_matrix(frame), frame->color_range == AVCOL_RANGE_JPEG, {-1, -1}, {0}, {0}, {0}};
  for (int i = 0; usable && i < 2; i++) {
    uint32_t object = planes.layers[i].object_index[0];
    held->fds[i] = out->fds[i] = dup(planes.objects[object].fd);
    out->offsets[i] = planes.layers[i].offset[0];
    out->pitches[i] = planes.layers[i].pitch[0];
    out->modifiers[i] = planes.objects[object].drm_format_modifier;
    usable = held->fds[i] >= 0;
  }
  for (uint32_t i = 0; i < planes.num_objects; i++) close(planes.objects[i].fd);
  if (!usable) apps_ffmpeg_release(held);
  return usable;
#else
  return 0;
#endif
}

// Decodes the rest of the video in software, while the audio goes on.
static void use_software(AppsMedia *m) {
  AVCodecContext *codec = m->video.codec;
  int64_t time = m->picture->best_effort_timestamp;
  m->software_only = 1;
  // FFmpeg decodes AV1 only in hardware.
  if (codec->codec_id == AV_CODEC_ID_AV1 || time == AV_NOPTS_VALUE ||
      !open_track(m, &m->video, AVMEDIA_TYPE_VIDEO))
    return;
  if (avformat_seek_file(m->format, m->video.stream->index, INT64_MIN, time, time, 0) < 0) {
    avcodec_free_context(&m->video.codec);
    m->video.codec = codec;
    return;
  }
  avcodec_free_context(&codec);
  av_buffer_unref(&m->device);
  clear(&m->video);
  m->read_end = 0;
  m->video_resume = time;
  m->audio_resume = m->audio_read;
}

int apps_ffmpeg_pixels(AppsMedia *m, uint8_t *pixels) {
  AVFrame *frame = m->picture;
  if (frame->format == m->hardware && frame->hw_frames_ctx) {
    av_frame_unref(m->software);
    int64_t start = av_gettime_relative();
    int status = av_hwframe_transfer_data(m->software, frame, 0);
    if (status >= 0) av_frame_copy_props(m->software, frame);
    if (frame->format == AV_PIX_FMT_VAAPI && !m->software_only) {
      // Some VA-API drivers give memory that the CPU reads slower than the video plays.
      AVRational rate = av_guess_frame_rate(m->format, m->video.stream, NULL);
      double limit = rate.num > 0 && rate.den > 0 ? 0.5 / (av_q2d(rate) * m->speed) : 1 / 60.;
      m->slow = (av_gettime_relative() - start) / 1e6 > limit ? m->slow + 1 : 0;
      if (status < 0 || m->slow == 4) use_software(m);
    }
    if (status < 0) return status;
    frame = m->software;
  }
  return convert(&m->scale, frame, m->width, m->height, pixels);
}

int apps_ffmpeg_seek(AppsMedia *m, double time) {
  int64_t start = m->format->start_time == AV_NOPTS_VALUE ? 0 : m->format->start_time;
  // Guard both the double-to-int64_t conversion and signed addition against overflow.
  int64_t target = (int64_t)(fmin(fmax(time, 0), 1e9) * AV_TIME_BASE);
  target = start > INT64_MAX - target ? INT64_MAX : target + start;
  int status = avformat_seek_file(m->format, -1, INT64_MIN, target, target, 0);
  if (status < 0) status = avformat_seek_file(m->format, -1, INT64_MIN, target, INT64_MAX, 0);
  clear(&m->video);
  clear(&m->audio);
  av_frame_unref(m->picture);
  m->read_end = 0;
  m->video_resume = m->audio_resume = m->audio_read = INT64_MIN;
  swr_free(&m->resample);
  return status;
}

void apps_ffmpeg_speed(AppsMedia *m, double speed) {
  if (speed == m->speed) return;
  m->speed = speed;
  swr_free(&m->resample);
}

void apps_ffmpeg_close(AppsMedia *m) {
  clear(&m->video);
  clear(&m->audio);
  av_free(m->video.queue);
  av_free(m->audio.queue);
  avcodec_free_context(&m->video.codec);
  avcodec_free_context(&m->audio.codec);
  avformat_close_input(&m->format);
  if (m->io) av_freep(&m->io->buffer);
  avio_context_free(&m->io);
  av_frame_free(&m->frame);
  av_frame_free(&m->software);
  av_frame_free(&m->picture);
  av_packet_free(&m->packet);
  av_buffer_unref(&m->device);
  av_buffer_unref(&m->surfaces);
  sws_freeContext(m->scale);
  swr_free(&m->resample);
  av_channel_layout_uninit(&m->layout);
  av_channel_layout_uninit(&m->input);
  av_free(m->samples);
  av_free(m);
}
