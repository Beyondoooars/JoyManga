<script setup lang="ts">
import AppContent from './AppContent.vue'
import type { GlobalThemeOverrides } from 'naive-ui'

// 与 src/styles.css 的设计令牌一一对应：主色只承担"选中/主操作"，
// 信息类状态（下载、进度）用蓝色，其余交互交给中性色。
const themeOverrides: GlobalThemeOverrides = {
  common: {
    primaryColor: '#FB7299',
    primaryColorHover: '#FF8CB0',
    primaryColorPressed: '#E45B84',
    primaryColorSuppl: '#FFB2C9',
    infoColor: '#00AEEC',
    infoColorHover: '#33BDEC',
    infoColorPressed: '#009CD3',
    successColor: '#1F9D63',
    warningColor: '#D98300',
    errorColor: '#DE3B3B',
    textColorBase: '#17181C',
    textColor1: '#17181C',
    textColor2: '#565C66',
    textColor3: '#8A9099',
    textColorDisabled: '#B3B8C0',
    placeholderColor: '#B3B8C0',
    bodyColor: '#F5F6F8',
    cardColor: '#FFFFFF',
    modalColor: '#FFFFFF',
    popoverColor: '#FFFFFF',
    tableColor: '#FFFFFF',
    borderColor: '#E8EAEE',
    dividerColor: '#F1F2F4',
    borderRadius: '12px',
    borderRadiusSmall: '9px',
    heightMedium: '44px',
    fontSizeMedium: '14px',
    fontFamily:
      "'Noto Sans SC', 'PingFang SC', 'Microsoft YaHei', Inter, 'SF Pro Display', system-ui, -apple-system, sans-serif",
  },
  Button: {
    colorPrimary: '#FB7299',
    colorHoverPrimary: '#FF8CB0',
    colorPressedPrimary: '#E45B84',
    colorFocusPrimary: '#FF8CB0',
    textColorPrimary: '#FFFFFF',
    textColorHoverPrimary: '#FFFFFF',
    textColorPressedPrimary: '#FFFFFF',
    colorSecondary: '#FFF0F5',
    colorHoverSecondary: '#FFE3EC',
    colorPressedSecondary: '#FFD6E3',
    textColorSecondary: '#D9557F',
    colorTertiary: '#F1F2F4',
    textColorTertiary: '#565C66',
    border: '1px solid #E8EAEE',
    borderHover: '1px solid #DDE0E5',
    borderFocus: '1px solid #FB7299',
    paddingSmall: '0 12px',
    paddingMedium: '0 18px',
    fontWeight: '600',
    heightSmall: '34px',
    heightMedium: '44px',
    heightLarge: '50px',
    borderRadiusSmall: '10px',
    borderRadiusMedium: '12px',
  },
  Input: {
    color: '#F1F2F4',
    colorFocus: '#FFFFFF',
    border: '1px solid transparent',
    borderHover: '1px solid #DDE0E5',
    borderFocus: '1px solid #FB7299',
    boxShadowFocus: '0 0 0 3px rgba(251, 114, 153, 0.14)',
    borderRadius: '12px',
    heightMedium: '44px',
    heightSmall: '38px',
    textColor: '#17181C',
    placeholderColor: '#8A9099',
    caretColor: '#FB7299',
  },
  Select: {
    peers: {
      InternalSelection: {
        color: '#F1F2F4',
        colorActive: '#FFFFFF',
        border: '1px solid transparent',
        borderHover: '1px solid #DDE0E5',
        borderActive: '1px solid #FB7299',
        borderFocus: '1px solid #FB7299',
        boxShadowActive: '0 0 0 3px rgba(251, 114, 153, 0.14)',
        boxShadowFocus: '0 0 0 3px rgba(251, 114, 153, 0.14)',
        borderRadius: '12px',
        heightMedium: '44px',
        heightSmall: '38px',
        textColor: '#17181C',
        placeholderColor: '#8A9099',
        arrowColor: '#8A9099',
      },
      InternalSelectMenu: {
        borderRadius: '14px',
        optionTextColorActive: '#FB7299',
        optionColorPending: 'rgba(251, 114, 153, 0.08)',
        optionColorActive: 'rgba(251, 114, 153, 0.1)',
        optionHeightMedium: '42px',
      },
    },
  },
  Radio: {
    buttonColorActive: '#FB7299',
    buttonTextColorActive: '#FFFFFF',
    buttonBorderColorActive: '#FB7299',
  },
  Checkbox: {
    colorChecked: '#FB7299',
    borderChecked: '1px solid #FB7299',
    borderFocus: '1px solid #FB7299',
    boxShadowFocus: '0 0 0 3px rgba(251, 114, 153, 0.16)',
    sizeMedium: '20px',
    borderRadius: '6px',
  },
  Switch: {
    railColorActive: '#FB7299',
    railColor: '#DDE0E5',
    buttonColor: '#FFFFFF',
    boxShadowFocus: '0 0 0 3px rgba(251, 114, 153, 0.16)',
  },
  Slider: {
    fillColor: '#FB7299',
    fillColorHover: '#FF8CB0',
    railColor: '#EDEFF2',
    railColorHover: '#E4E7EB',
    handleColor: '#FFFFFF',
    handleBoxShadow: '0 1px 4px rgba(23, 24, 28, 0.24)',
  },
  Progress: {
    fillColor: '#FB7299',
    railColor: '#EFF1F4',
    textColorLineInner: '#17181C',
  },
  Tabs: {
    tabFontWeightActive: '700',
    tabTextColorActiveLine: '#FB7299',
    tabTextColorHoverLine: '#FF8CB0',
    tabTextColorLine: '#565C66',
    barColor: '#FB7299',
    tabPaddingMediumLine: '12px 0',
  },
  Pagination: {
    itemColorActive: '#FB7299',
    itemColorActiveHover: '#FF8CB0',
    itemColorHover: 'rgba(251, 114, 153, 0.1)',
    itemTextColorActive: '#FFFFFF',
    itemTextColorHover: '#FB7299',
    itemBorderRadius: '10px',
  },
  Dropdown: {
    borderRadius: '14px',
    padding: '6px 4px',
    color: '#FFFFFF',
    optionColorHover: 'rgba(251, 114, 153, 0.08)',
    optionTextColorHover: '#FB7299',
    optionHeightMedium: '42px',
    optionFontSizeMedium: '14px',
  },
  Card: {
    color: '#FFFFFF',
    borderRadius: '16px',
    borderColor: '#E8EAEE',
  },
  Modal: {
    borderRadius: '20px',
  },
  Dialog: {
    borderRadius: '18px',
  },
  Tag: {
    borderRadius: '8px',
  },
  Alert: {
    borderRadius: '12px',
  },
  Message: {
    borderRadius: '12px',
  },
  Notification: {
    borderRadius: '14px',
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
