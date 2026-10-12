# Let PipeWire follow the sample rate of what is actually playing.
#
# Paste this block into /etc/nixos/configuration.nix, then:
#   sudo nixos-rebuild switch
#
# ---------------------------------------------------------------------------
# DO NOT BOTHER ON THIS LAPTOP. Measured 2026-10-12; keep this file for an
# external DAC, which is the only case where it can do anything.
#
# This does widen the graph: with the rates below allowed, the driver node
# genuinely runs at 44100 and 96000. But every sink this ThinkPad has —
# Speaker and all three HDMI — advertises exactly one rate in its `EnumFormat`,
# 48000, because the `sof-hda-dsp` topology pins the PCM there. So the ALSA
# sink just resamples instead of the graph adapter. One resample either way.
#
# And it is inaudible regardless. A 44.1k -> 48k sweep through PipeWire's
# resampler at the default `resample.quality = 4` measured flat to ±0.00 dB
# from 100 Hz to 15 kHz (-0.18 dB to 18 kHz), with aliasing images at -97 dB —
# below the noise floor of the 16-bit material most of this collection is.
# ---------------------------------------------------------------------------
#
# Why it was written: PipeWire ships with `clock.allowed-rates = [ 48000 ]`,
# verified as the live setting on this machine. hark! already asks the device
# for the file's native rate (`alsa_playback.hark` registers at 1/44100 in
# `pw-dump`), but PipeWire can only honour that if the rate is on this list.
# The extra rates cover the 24-bit and 88.2/96kHz FLACs in the collection.
#
# Caveat worth knowing on hardware where this *does* help: the graph rate is
# global. If a Chrome tab is holding a 48kHz stream open, 44.1kHz material
# still gets resampled while it plays. This setting makes native-rate playback
# possible, not guaranteed.
#
# To try it without rebuilding (resets when PipeWire restarts):
#   pw-metadata -n settings 0 clock.allowed-rates '[ 44100, 48000, 88200, 96000 ]'

{
  services.pipewire.extraConfig.pipewire."92-hark-rates" = {
    "context.properties" = {
      "default.clock.rate" = 48000;
      "default.clock.allowed-rates" = [ 44100 48000 88200 96000 ];
    };
  };
}
