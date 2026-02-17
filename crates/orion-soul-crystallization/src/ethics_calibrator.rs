//! Maps MentorProfile to ethic weights (Triangle and Compass).

use crate::models::{AttachmentStyle, MentorProfile, TriangleEthicWeights};
use genesis_core::CompassEthicWeights;

/// Calibrate Triangle Ethic weights from a MentorProfile (legacy, 3 dimensions).
pub fn calibrate_triangle_ethic(profile: &MentorProfile) -> TriangleEthicWeights {
    let mf = &profile.moral_foundations;
    let ocean = &profile.ocean;

    let mut deontological = (mf.authority + mf.sanctity) / 2.0;
    let mut areteological = (mf.care + mf.fairness) / 2.0;
    let mut teleological = (mf.liberty + mf.loyalty) / 2.0;

    let ocean_scale = 0.1;
    deontological += (ocean.conscientiousness - 0.5) * ocean_scale;
    areteological += (ocean.agreeableness - 0.5) * ocean_scale;
    teleological += (ocean.openness - 0.5) * ocean_scale;

    let attachment_scale = 0.03;
    match profile.attachment_style {
        AttachmentStyle::Secure => {
            areteological += attachment_scale;
        }
        AttachmentStyle::Anxious => {
            deontological += attachment_scale;
        }
        AttachmentStyle::Avoidant => {
            teleological += attachment_scale;
        }
        AttachmentStyle::Disorganized => {}
    }

    deontological = deontological.max(0.01);
    areteological = areteological.max(0.01);
    teleological = teleological.max(0.01);

    let mut weights = TriangleEthicWeights {
        deontological,
        areteological,
        teleological,
    };
    weights.normalize();
    weights
}

/// Calibrate 4-dimension Compass Ethic weights from a MentorProfile.
///
/// Formula:
/// - duty = (authority + sanctity) / 2 + conscientiousness modulation + anxious attachment
/// - virtue = (liberty + sanctity) / 2 + openness modulation + (no attachment)
/// - outcome = (loyalty + liberty) / 2 + extraversion modulation + avoidant attachment
/// - welfare = (care + fairness) / 2 + agreeableness modulation + secure attachment
pub fn calibrate_compass_ethic(profile: &MentorProfile) -> CompassEthicWeights {
    let mf = &profile.moral_foundations;
    let ocean = &profile.ocean;

    let mut duty = (mf.authority + mf.sanctity) / 2.0;
    let mut virtue = (mf.liberty + mf.sanctity) / 2.0;
    let mut outcome = (mf.loyalty + mf.liberty) / 2.0;
    let mut welfare = (mf.care + mf.fairness) / 2.0;

    let ocean_scale = 0.1;
    duty += (ocean.conscientiousness - 0.5) * ocean_scale;
    virtue += (ocean.openness - 0.5) * ocean_scale;
    outcome += (ocean.extraversion - 0.5) * ocean_scale;
    welfare += (ocean.agreeableness - 0.5) * ocean_scale;

    let attachment_scale = 0.03;
    match profile.attachment_style {
        AttachmentStyle::Secure => {
            welfare += attachment_scale;
        }
        AttachmentStyle::Anxious => {
            duty += attachment_scale;
        }
        AttachmentStyle::Avoidant => {
            outcome += attachment_scale;
        }
        AttachmentStyle::Disorganized => {}
    }

    duty = duty.max(0.01);
    virtue = virtue.max(0.01);
    outcome = outcome.max(0.01);
    welfare = welfare.max(0.01);

    let mut weights = CompassEthicWeights {
        duty,
        virtue,
        outcome,
        welfare,
    };
    weights.normalize();
    weights
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{DepthLevel, MentorProfile};

    #[test]
    fn test_calibrate_triangle_neutral_profile() {
        let profile = MentorProfile::new(DepthLevel::Conversation);
        let w = calibrate_triangle_ethic(&profile);
        // Neutral moral foundations (all 0.5) + neutral OCEAN → roughly equal
        assert!((w.deontological + w.areteological + w.teleological - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_calibrate_compass_neutral_profile() {
        let profile = MentorProfile::new(DepthLevel::Conversation);
        let w = calibrate_compass_ethic(&profile);
        assert!(w.is_valid());
        // All roughly equal for neutral profile
        assert!((w.duty - 0.25).abs() < 0.05);
        assert!((w.virtue - 0.25).abs() < 0.05);
        assert!((w.outcome - 0.25).abs() < 0.05);
        assert!((w.welfare - 0.25).abs() < 0.05);
    }

    #[test]
    fn test_calibrate_compass_high_authority() {
        let mut profile = MentorProfile::new(DepthLevel::Conversation);
        profile.moral_foundations.authority = 0.9;
        profile.moral_foundations.sanctity = 0.8;
        let w = calibrate_compass_ethic(&profile);
        assert!(w.is_valid());
        assert_eq!(w.dominant(), "duty");
    }

    #[test]
    fn test_calibrate_compass_high_care() {
        let mut profile = MentorProfile::new(DepthLevel::Conversation);
        profile.moral_foundations.care = 0.9;
        profile.moral_foundations.fairness = 0.8;
        let w = calibrate_compass_ethic(&profile);
        assert!(w.is_valid());
        assert_eq!(w.dominant(), "welfare");
    }

    #[test]
    fn test_from_triangle_migration() {
        let profile = MentorProfile::new(DepthLevel::Conversation);
        let triangle = calibrate_triangle_ethic(&profile);
        let migrated = CompassEthicWeights::from_triangle(
            triangle.deontological,
            triangle.areteological,
            triangle.teleological,
        );
        assert!(migrated.is_valid());
    }
}
