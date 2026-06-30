import React from "react";
import { useTranslation } from "react-i18next";
import { useSettings } from "../../hooks/useSettings";
import { Select } from "../ui/Select";
import { SettingContainer } from "../ui/SettingContainer";

interface DictationStabilityProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const DictationStability: React.FC<DictationStabilityProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const mode = getSetting("dictation_stability_mode") ?? "stable_live";

    const options = [
      {
        value: "stable_live",
        label: t("settings.general.dictationStability.options.stableLive"),
      },
      {
        value: "fast_live",
        label: t("settings.general.dictationStability.options.fastLive"),
      },
      {
        value: "final_only",
        label: t("settings.general.dictationStability.options.finalOnly"),
      },
    ];

    return (
      <SettingContainer
        title={t("settings.general.dictationStability.title")}
        description={t("settings.general.dictationStability.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      >
        <Select
          value={mode}
          options={options}
          isClearable={false}
          disabled={isUpdating("dictation_stability_mode")}
          onChange={(value) => {
            if (value) {
              updateSetting("dictation_stability_mode", value as any);
            }
          }}
          className="min-w-44"
        />
      </SettingContainer>
    );
  },
);

DictationStability.displayName = "DictationStability";
