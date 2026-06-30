import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface GameModeAutoUnloadProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const GameModeAutoUnload: React.FC<GameModeAutoUnloadProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const enabled = getSetting("game_mode_auto_unload") ?? false;

    return (
      <ToggleSwitch
        checked={enabled}
        onChange={(value) => updateSetting("game_mode_auto_unload", value)}
        isUpdating={isUpdating("game_mode_auto_unload")}
        label={t("settings.advanced.gameMode.label")}
        description={t("settings.advanced.gameMode.description")}
        descriptionMode={descriptionMode}
        grouped={grouped}
      />
    );
  },
);
