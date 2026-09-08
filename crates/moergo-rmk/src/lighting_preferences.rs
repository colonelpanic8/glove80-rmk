use rmk::lighting::compositor::LightingSource;
use rmk::lighting::{LightingContext, Rgb8};
use rmk::storage::LightingExtensionParamsRecord;

pub fn restore_params<S: LightingSource<Rgb8, LightingContext>>(
    source: &mut S,
    primary: u8,
    overlay: Option<u8>,
    default_effect: u8,
    defaults: &[u8],
    records: &[LightingExtensionParamsRecord],
) {
    // PaletteFxConfig seeds only the selected bands. Preserve board defaults
    // for an inactive default effect too, then apply durable per-effect tuning.
    if primary != default_effect && overlay != Some(default_effect) {
        for (index, value) in defaults.iter().copied().enumerate() {
            source.apply_extension_param(default_effect, index as u8, value);
        }
    }
    for record in records {
        for (index, value) in record.values[..usize::from(record.len)]
            .iter()
            .copied()
            .enumerate()
        {
            source.apply_extension_param(record.effect, record.offset + index as u8, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmk_palettefx::effects::Effect;
    use rmk_palettefx::layout::SliceLayout;
    use rmk_palettefx::rmk_lighting::{HitQueue, PaletteFxConfig, PaletteFxSource};

    static HITS: HitQueue<1> = HitQueue::new();
    const CROSSHAIR: u8 = 7;
    const DEFAULTS: [u8; 7] = [0, 90, 11, 170, 1, 0, 173];

    fn restored(
        primary: u8,
        overlay: Option<u8>,
        records: &[LightingExtensionParamsRecord],
    ) -> Vec<u8> {
        let mut source = PaletteFxSource::<_, 1, 1>::new(
            SliceLayout::new(&[(128, 128)]),
            &HITS,
            PaletteFxConfig {
                initial_effect: primary,
                initial_overlay: overlay,
                initial_params: [1, 42, 9, 100, 2, 17, 18, 0],
                initial_param_len: 7,
                initial_overlay_params: [1, 42, 9, 100, 2, 17, 18, 0],
                initial_overlay_param_len: 7,
                ..PaletteFxConfig::default()
            },
        );
        restore_params(&mut source, primary, overlay, CROSSHAIR, &DEFAULTS, records);
        (0..7)
            .map(|index| {
                <_ as LightingSource<Rgb8, LightingContext>>::extension_param(
                    &source, CROSSHAIR, index,
                )
                .unwrap()
            })
            .collect()
    }

    #[test]
    fn inactive_compiled_effect_keeps_its_board_defaults_after_selection_restore() {
        assert_eq!(restored(Effect::<1>::RAIN_INDEX, None, &[]), DEFAULTS);
    }

    #[test]
    fn legacy_selected_and_overlay_tuning_take_precedence_over_board_defaults() {
        let tuned = [1, 42, 9, 100, 2, 17, 18];
        assert_eq!(restored(CROSSHAIR, None, &[]), tuned);
        assert_eq!(
            restored(Effect::<1>::RAIN_INDEX, Some(CROSSHAIR), &[]),
            tuned
        );
    }

    #[test]
    fn durable_inactive_effect_tuning_overrides_compiled_defaults() {
        let mut record = LightingExtensionParamsRecord {
            effect: CROSSHAIR,
            offset: 0,
            len: 7,
            values: [0; rmk::types::protocol::rynk::LIGHTING_EXTENSION_PARAM_CHUNK],
        };
        record.values[..7].copy_from_slice(&[1, 42, 9, 100, 2, 17, 18]);
        assert_eq!(
            restored(Effect::<1>::RAIN_INDEX, None, &[record]),
            [1, 42, 9, 100, 2, 17, 18]
        );
    }
}
