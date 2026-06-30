import React, { useState } from "react";
import { X } from "lucide-react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { useSettings } from "../../hooks/useSettings";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { SettingContainer } from "../ui/SettingContainer";
import { ToggleSwitch } from "../ui/ToggleSwitch";

interface VoiceFinishTriggerProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

const sanitizePhrase = (value: string) =>
  value
    .trim()
    .replace(/[<>"'&]/g, "")
    .replace(/\s+/g, " ");

const PhraseList: React.FC<{
  phrases: string[];
  settingKey: "voice_finish_phrases" | "voice_finish_phrase_variants";
  placeholder: string;
  addLabel: string;
  duplicateMessage: (phrase: string) => string;
  removeLabel: (phrase: string) => string;
  disabled: boolean;
}> = ({
  phrases,
  settingKey,
  placeholder,
  addLabel,
  duplicateMessage,
  removeLabel,
  disabled,
}) => {
  const { updateSetting } = useSettings();
  const [newPhrase, setNewPhrase] = useState("");

  const handleAdd = () => {
    const sanitized = sanitizePhrase(newPhrase);
    if (!sanitized || sanitized.length > 80) {
      return;
    }

    const exists = phrases.some(
      (phrase) => phrase.toLowerCase() === sanitized.toLowerCase(),
    );
    if (exists) {
      toast.error(duplicateMessage(sanitized));
      return;
    }

    updateSetting(settingKey, [...phrases, sanitized]);
    setNewPhrase("");
  };

  const handleRemove = (phraseToRemove: string) => {
    updateSetting(
      settingKey,
      phrases.filter((phrase) => phrase !== phraseToRemove),
    );
  };

  return (
    <div className="space-y-2">
      <div className="flex items-center gap-2">
        <Input
          type="text"
          className="min-w-0 flex-1"
          value={newPhrase}
          onChange={(event) => setNewPhrase(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault();
              handleAdd();
            }
          }}
          placeholder={placeholder}
          variant="compact"
          disabled={disabled}
        />
        <Button
          onClick={handleAdd}
          disabled={
            !newPhrase.trim() || newPhrase.trim().length > 80 || disabled
          }
          variant="primary"
          size="md"
        >
          {addLabel}
        </Button>
      </div>
      {phrases.length > 0 && (
        <div className="flex flex-wrap gap-1">
          {phrases.map((phrase) => (
            <Button
              key={phrase}
              onClick={() => handleRemove(phrase)}
              disabled={disabled}
              variant="secondary"
              size="sm"
              className="inline-flex items-center gap-1"
              aria-label={removeLabel(phrase)}
            >
              <span>{phrase}</span>
              <X className="h-3 w-3" aria-hidden="true" />
            </Button>
          ))}
        </div>
      )}
    </div>
  );
};

export const VoiceFinishTrigger: React.FC<VoiceFinishTriggerProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const enabled = getSetting("voice_finish_trigger_enabled") ?? false;
    const phrases = getSetting("voice_finish_phrases") ?? [];
    const variants = getSetting("voice_finish_phrase_variants") ?? [];

    return (
      <>
        <ToggleSwitch
          checked={enabled}
          onChange={(checked) =>
            updateSetting("voice_finish_trigger_enabled", checked)
          }
          isUpdating={isUpdating("voice_finish_trigger_enabled")}
          label={t("settings.general.voiceFinish.label")}
          description={t("settings.general.voiceFinish.description")}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
        {enabled && (
          <SettingContainer
            title={t("settings.general.voiceFinish.phrases.title")}
            description={t("settings.general.voiceFinish.phrases.description")}
            descriptionMode={descriptionMode}
            grouped={grouped}
            layout="stacked"
          >
            <PhraseList
              phrases={phrases}
              settingKey="voice_finish_phrases"
              placeholder={t(
                "settings.general.voiceFinish.phrases.placeholder",
              )}
              addLabel={t("settings.general.voiceFinish.add")}
              duplicateMessage={(phrase) =>
                t("settings.general.voiceFinish.duplicate", { phrase })
              }
              removeLabel={(phrase) =>
                t("settings.general.voiceFinish.remove", { phrase })
              }
              disabled={isUpdating("voice_finish_phrases")}
            />
          </SettingContainer>
        )}
        {enabled && (
          <SettingContainer
            title={t("settings.general.voiceFinish.variants.title")}
            description={t("settings.general.voiceFinish.variants.description")}
            descriptionMode={descriptionMode}
            grouped={grouped}
            layout="stacked"
          >
            <PhraseList
              phrases={variants}
              settingKey="voice_finish_phrase_variants"
              placeholder={t(
                "settings.general.voiceFinish.variants.placeholder",
              )}
              addLabel={t("settings.general.voiceFinish.add")}
              duplicateMessage={(phrase) =>
                t("settings.general.voiceFinish.duplicate", { phrase })
              }
              removeLabel={(phrase) =>
                t("settings.general.voiceFinish.remove", { phrase })
              }
              disabled={isUpdating("voice_finish_phrase_variants")}
            />
          </SettingContainer>
        )}
      </>
    );
  },
);

VoiceFinishTrigger.displayName = "VoiceFinishTrigger";
