<script setup lang="ts">
import AppContent from './AppContent.vue'
import { GlobalThemeOverrides } from 'naive-ui'

const themeOverrides: GlobalThemeOverrides = {
  common: {
    primaryColor: '#FB7299',
    primaryColorHover: '#FF85AD',
    primaryColorPressed: '#E05D84',
    primaryColorSuppl: '#FFB2C9',
    infoColor: '#00AEEC',
    infoColorHover: '#33BDEC',
    infoColorPressed: '#009CD3',
    textColorBase: '#18191C',
    textColor1: '#18191C',
    textColor2: '#61666D',
    textColor3: '#9499A0',
    bodyColor: '#F6F7F8',
    cardColor: '#FFFFFF',
    modalColor: '#FFFFFF',
    popoverColor: '#FFFFFF',
    borderColor: '#E3E5E7',
    dividerColor: '#F1F2F3',
    borderRadius: '10px',
    borderRadiusSmall: '6px',
    heightMedium: '38px',
  },
  Button: {
    colorPrimary: '#FB7299',
    colorHoverPrimary: '#FF85AD',
    colorPressedPrimary: '#E05D84',
    colorFocusPrimary: '#FF85AD',
    textColorPrimary: '#FFFFFF',
    paddingSmall: '0 12px',
    paddingMedium: '0 16px',
    fontWeight: '600',
    heightSmall: '32px',
    borderRadiusSmall: '8px',
    borderRadiusMedium: '10px',
  },
  Radio: {
    buttonColorActive: '#FB7299',
    buttonTextColorActive: '#FFF',
  },
  Dropdown: {
    borderRadius: '12px',
    padding: '6px 4px',
    color: '#FFFFFF',
    optionColorHover: 'rgba(251, 114, 153, 0.08)',
    optionTextColorHover: '#FB7299',
    optionHeightMedium: '34px',
  },
  Card: {
    color: '#FFFFFF',
    borderRadius: '16px',
    borderColor: '#E3E5E7',
  },
  Tabs: {
    tabFontWeightActive: '700',
    tabTextColorActiveLine: '#FB7299',
    tabTextColorHoverLine: '#FF85AD',
    barColor: '#FB7299',
  },
  Input: {
    color: '#F1F2F3',
    colorFocus: '#FFFFFF',
    borderColor: '#E3E5E7',
    borderHover: '#FB7299',
    borderFocus: '#FB7299',
    borderRadius: '10px',
    textColor: '#18191C',
    placeholderColor: '#9499A0',
  },
}
</script>

<template>
  <n-config-provider :theme-overrides="themeOverrides">
    <n-modal-provider>
      <n-dialog-provider>
        <n-notification-provider placement="bottom-right" :max="3">
          <n-message-provider>
            <app-content />
          </n-message-provider>
        </n-notification-provider>
      </n-dialog-provider>
    </n-modal-provider>
  </n-config-provider>
</template>
