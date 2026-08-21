# Doubao Lala-land Sequence + Step Transition Audit

- Before: archive revision 6239, 1820 frames.
- After: archive revision 6384, 1820 frames.
- Coverage: 20 persisted Transition edges; 0 physical identity violations.
- Exact unchanged channels: 27 channels across every frame.
- Effect targets: pulse Q exact on every frame; wave Q exact on every frame; max wave Qdot delta 2.143e-4. Phase Q differs on 138 frames only because 收起 - 2 now authors phase 0.3 instead of inheriting the prior Mutation cursor.
- Intentional physical differences: pulse P differs on 730 frames because its Transition route is now Step/Instant and the stale-time activation residual is gone; wave P differs on 842 frames because its Transition route is now Step/Instant.
- Step invariant: phase/pulse/wave E and Edot are zero on every sampled frame where those channels exist, so Transition never interpolates those Mutation outputs.

`Step` below means an instantaneous cut on the activation sample. It is intentionally reported as a discontinuity, not as an interpolated handoff.

| Transition     | State switch                  | Before discontinuities                         | After discontinuities                                                 | Phase route after | Pulse route after |   Wave before → after | Wave route after |
| -------------- | ----------------------------- | ---------------------------------------------- | --------------------------------------------------------------------- | ----------------- | ----------------- | --------------------: | ---------------- |
| tr_msx1vbpw_s  | any → collapsed               | —                                              | —                                                                     | not eligible      | not eligible      |                 — → — | not eligible     |
| tr_mszxk0y7_k  | collapsed → supercharge       | —                                              | —                                                                     | Step (E=0)        | Step (E=0)        |          — → 2.285969 | Step (E=0)       |
| tr_mszxk2bk_n  | collapsed → st_msybtf2o_m     | sp_effect_pulse_gain, sp_effect_wave_radius_dp | sp_effect_cycle_phase, sp_effect_pulse_gain, sp_effect_wave_radius_dp | Step (E=0)        | Step (E=0)        |  39.870425 → 2.285969 | Step (E=0)       |
| tr_mszxk3wu_q  | collapsed → island            | sp_effect_pulse_gain, sp_effect_wave_radius_dp | sp_effect_cycle_phase, sp_effect_pulse_gain, sp_effect_wave_radius_dp | Step (E=0)        | Step (E=0)        |         40 → 2.285969 | Step (E=0)       |
| tr_mt1aavwx_22 | st_msybtf2o_m → island        | sp_effect_pulse_gain, sp_effect_wave_radius_dp | sp_effect_cycle_phase, sp_effect_pulse_gain, sp_effect_wave_radius_dp | Step (E=0)        | Step (E=0)        |  39.870425 → 2.285969 | Step (E=0)       |
| tr_mt1aax6i_25 | supercharge → island          | —                                              | sp_effect_cycle_phase, sp_effect_pulse_gain, sp_effect_wave_radius_dp | Step (E=0)        | Step (E=0)        |         40 → 2.285969 | Step (E=0)       |
| tr_mt1abe43_28 | island → st_msybtf2o_m        | —                                              | sp_effect_cycle_phase, sp_effect_pulse_gain, sp_effect_wave_radius_dp | Step (E=0)        | Step (E=0)        |  39.827784 → 2.285969 | Step (E=0)       |
| tr_mt1abkhf_2b | supercharge → st_msybtf2o_m   | sp_effect_pulse_gain, sp_effect_wave_radius_dp | sp_effect_cycle_phase, sp_effect_pulse_gain, sp_effect_wave_radius_dp | Step (E=0)        | Step (E=0)        |  39.870425 → 2.285969 | Step (E=0)       |
| tr_mt1abve2_2e | island → supercharge          | sp_effect_pulse_gain, sp_effect_wave_radius_dp | sp_effect_cycle_phase, sp_effect_pulse_gain, sp_effect_wave_radius_dp | Step (E=0)        | Step (E=0)        |  39.827784 → 2.285969 | Step (E=0)       |
| tr_mt1abynw_2h | st_msybtf2o_m → supercharge   | sp_effect_pulse_gain, sp_effect_wave_radius_dp | sp_effect_cycle_phase, sp_effect_pulse_gain, sp_effect_wave_radius_dp | Step (E=0)        | Step (E=0)        |  39.870425 → 2.285969 | Step (E=0)       |
| tr_mt1ajfjb_10 | st_mt1ajfjb_p → st_mt1ajfjb_r | sp_effect_cycle_phase                          | sp_effect_cycle_phase, sp_effect_pulse_gain                           | Step (E=0)        | Step (E=0)        |               40 → 40 | Step (E=0)       |
| tr_mt1ajfjb_11 | st_mt1ajfjb_s → st_mt1ajfjb_r | sp_effect_cycle_phase                          | sp_effect_cycle_phase, sp_effect_pulse_gain, sp_effect_wave_radius_dp | Step (E=0)        | Step (E=0)        |        35.795512 → 40 | Step (E=0)       |
| tr_mt1ajfjb_t  | st_mt1ajfjb_q → st_mt1ajfjb_r | —                                              | sp_effect_cycle_phase                                                 | Step (E=0)        | Step (E=0)        |                — → 40 | Step (E=0)       |
| tr_mt1ajfjb_u  | st_mt1ajfjb_q → st_mt1ajfjb_s | sp_effect_cycle_phase                          | sp_effect_cycle_phase, sp_effect_pulse_gain, sp_effect_wave_radius_dp | Step (E=0)        | Step (E=0)        |        35.795512 → 40 | Step (E=0)       |
| tr_mt1ajfjb_v  | st_mt1ajfjb_q → st_mt1ajfjb_p | sp_effect_cycle_phase                          | sp_effect_pulse_gain, sp_effect_wave_radius_dp                        | Step (E=0)        | Step (E=0)        | 37.553998 → 29.345384 | Step (E=0)       |
| tr_mt1ajfjb_w  | st_mt1ajfjb_s → st_mt1ajfjb_p | sp_effect_cycle_phase                          | sp_effect_cycle_phase, sp_effect_pulse_gain, sp_effect_wave_radius_dp | Step (E=0)        | Step (E=0)        | 35.795512 → 29.345384 | Step (E=0)       |
| tr_mt1ajfjb_x  | st_mt1ajfjb_r → st_mt1ajfjb_p | sp_effect_cycle_phase                          | sp_effect_cycle_phase, sp_effect_pulse_gain, sp_effect_wave_radius_dp | Step (E=0)        | Step (E=0)        | 37.553998 → 29.345384 | Step (E=0)       |
| tr_mt1ajfjb_y  | st_mt1ajfjb_p → st_mt1ajfjb_s | sp_effect_cycle_phase                          | sp_effect_cycle_phase, sp_effect_pulse_gain                           | Step (E=0)        | Step (E=0)        |               40 → 40 | Step (E=0)       |
| tr_mt1ajfjb_z  | st_mt1ajfjb_r → st_mt1ajfjb_s | sp_effect_cycle_phase                          | sp_effect_cycle_phase, sp_effect_pulse_gain, sp_effect_wave_radius_dp | Step (E=0)        | Step (E=0)        |        35.795512 → 40 | Step (E=0)       |
| tr_mt1ak8yj_1b | any → st_mt1ajfjb_q           | —                                              | —                                                                     | Step (E=0)        | not eligible      |                 — → — | not eligible     |
