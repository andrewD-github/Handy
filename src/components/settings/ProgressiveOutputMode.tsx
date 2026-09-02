import React from "react";
import { useTranslation } from "react-i18next";
import type { ProgressiveOutputMode } from "@/bindings";
import { useSettings } from "../../hooks/useSettings";
import { Dropdown } from "../ui/Dropdown";
import { SettingContainer } from "../ui/SettingContainer";

interface ProgressiveOutputModeProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const ProgressiveOutputModeSetting: React.FC<ProgressiveOutputModeProps> =
  React.memo(({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();
    const selected = (getSetting("progressive_output_mode") ||
      "overlay") as ProgressiveOutputMode;

    return (
      <SettingContainer
        title={t("settings.advanced.progressiveOutput.title")}
        description={t("settings.advanced.progressiveOutput.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      >
        <Dropdown
          options={[
            {
              value: "overlay",
              label: t("settings.advanced.progressiveOutput.options.overlay"),
            },
            {
              value: "direct_prompt",
              label: t(
                "settings.advanced.progressiveOutput.options.directPrompt",
              ),
            },
          ]}
          selectedValue={selected}
          onSelect={(value) =>
            updateSetting(
              "progressive_output_mode",
              value as ProgressiveOutputMode,
            )
          }
          disabled={isUpdating("progressive_output_mode")}
        />
      </SettingContainer>
    );
  });
