use objc2_core_foundation::{CFArray, CFDictionary, CFNumber, CFRetained, CFString, CFType};
use objc2_core_text::{
    CTFont, CTFontDescriptor, CTFontTableOptions, kCTFontFeatureSettingsAttribute,
    kCTFontOpenTypeFeatureTag, kCTFontOpenTypeFeatureValue, kCTFontVariationAttribute,
    kCTFontVariationAxisDefaultValueKey, kCTFontVariationAxisIdentifierKey,
};
use std::ptr;

const WGHT: i64 = 0x77676874;

fn descriptor(key: &CFString, value: &CFType) -> CFRetained<CTFontDescriptor> {
    let attributes = CFDictionary::<CFString, CFType>::from_slices(&[key], &[value]);
    // SAFETY: the dictionary holds a valid font attribute.
    unsafe { CTFontDescriptor::with_attributes(attributes.as_opaque()) }
}

pub(super) fn with_features(font: &CTFont, features: &[(String, u32)]) -> CFRetained<CTFont> {
    let settings: Vec<_> = features
        .iter()
        .map(|(tag, value)| {
            let (tag, value) = (CFString::from_str(tag), CFNumber::new_i32(*value as i32));
            // SAFETY: the keys are CoreText constants.
            let keys = unsafe { [kCTFontOpenTypeFeatureTag, kCTFontOpenTypeFeatureValue] };
            CFDictionary::<CFString, CFType>::from_slices(&keys, &[&tag, &value])
        })
        .collect();
    let settings = CFArray::from_retained_objects(&settings);
    // SAFETY: a size of 0 keeps the font's size, and the descriptor only adds feature settings.
    unsafe {
        let attributes = descriptor(kCTFontFeatureSettingsAttribute, &settings);
        font.copy_with_attributes(0., ptr::null(), Some(&attributes))
    }
}

pub(super) fn with_weight(font: &CTFont, weight: f32) -> CFRetained<CTFont> {
    let axis = CFNumber::new_i64(WGHT);
    let value = CFNumber::new_f64(f64::from(weight));
    let variation = CFDictionary::<CFNumber, CFNumber>::from_slices(&[&axis], &[&value]);
    // SAFETY: the variation dictionary maps an axis identifier to a value, as CoreText expects.
    unsafe {
        let attributes = descriptor(kCTFontVariationAttribute, &variation);
        font.copy_with_attributes(font.size(), ptr::null(), Some(&attributes))
    }
}

pub(super) fn weight(font: &CTFont) -> Option<f32> {
    // SAFETY: CoreText returns a dictionary of numbers or nothing.
    let variation = unsafe { font.variation()? };
    let variation = unsafe { variation.cast_unchecked::<CFNumber, CFNumber>() };
    Some(variation.get(&CFNumber::new_i64(WGHT))?.as_f64()? as f32)
}

/// The default of the `wght` axis when it is on the CSS scale. Old Apple fonts like Skia use 0.5 to 3.2 and
/// keep their named faces.
pub(super) fn default_weight(font: &CTFont) -> Option<f32> {
    // SAFETY: CoreText returns an array of axis dictionaries or nothing.
    let axes = unsafe { font.variation_axes()? };
    let axes = unsafe { axes.cast_unchecked::<CFDictionary<CFString, CFType>>() };
    let number = |axis: &CFDictionary<CFString, CFType>, key: &CFString| {
        axis.get(key)?.downcast::<CFNumber>().ok()
    };
    axes.iter()
        .find_map(|axis| {
            // SAFETY: the keys are CoreText constants.
            let (id, default) = unsafe {
                (
                    kCTFontVariationAxisIdentifierKey,
                    kCTFontVariationAxisDefaultValueKey,
                )
            };
            (number(&axis, id)?.as_i64()? == WGHT).then(|| number(&axis, default)?.as_f32())?
        })
        .filter(|weight| *weight >= 100.)
}

fn os2(font: &CTFont, len: usize) -> Option<CFRetained<objc2_core_foundation::CFData>> {
    let tag = u32::from_be_bytes(*b"OS/2");
    // SAFETY: the table is copied out of the font. CoreText ignores the option to exclude synthetic tables.
    let table = unsafe { font.table(tag, CTFontTableOptions::NoOptions)? };
    (table.len() >= len).then_some(table)
}

/// `usWeightClass`. CoreText puts fonts loaded from data on another weight scale (0.2 for Medium), so their
/// matching reads the class.
pub(super) fn weight_class(font: &CTFont) -> Option<f32> {
    let table = os2(font, 6)?;
    let bytes = table.to_vec();
    Some(f32::from(u16::from_be_bytes([bytes[4], bytes[5]])))
        .filter(|class| (1. ..=1000.).contains(class))
}

pub(super) fn typo_metrics(font: &CTFont) -> Option<[f32; 2]> {
    let bytes = os2(font, 72)?.to_vec();
    Some([68, 70].map(|at| f32::from(i16::from_be_bytes([bytes[at], bytes[at + 1]]))))
        .filter(|[ascent, descent]| *ascent > 0. && *descent < 0.)
}
