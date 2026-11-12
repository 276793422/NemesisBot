import { describe, it, expect } from 'vitest'
import { useVariablePicker, type VariableOption } from '../useVariablePicker'

function makeInput(value: string, pos = value.length): HTMLTextAreaElement {
  const el = document.createElement('textarea')
  el.value = value
  el.selectionStart = el.selectionEnd = pos
  document.body.appendChild(el)
  return el
}

function typeInto(el: HTMLTextAreaElement, text: string) {
  el.value += text
  el.selectionStart = el.selectionEnd = el.value.length
}

const VARS: VariableOption[] = [
  { value: 'user_input', label: '用户输入', group: 'Workflow variables' },
  { value: 'fetch.output.text', label: '抓取结果', group: 'Node outputs' },
  { value: 'today', label: 'Today 日期' },
]

describe('useVariablePicker 触发', () => {
  it('空格后输入 @ 打开选择器并带锚点', () => {
    const el = makeInput('hello ')
    const { state, onInput } = useVariablePicker(() => el, () => VARS)
    typeInto(el, '@')
    onInput(new Event('input'))
    expect(state.value.open).toBe(true)
    expect(state.value.startIndex).toBe(el.value.length - 1)
    expect(state.value.anchor).toEqual({ top: expect.any(Number), left: expect.any(Number) })
    expect(state.value.matches.length).toBe(3)
  })

  it('行首 @ 直接触发（pos===1）', () => {
    const el = makeInput('')
    const { state, onInput } = useVariablePicker(() => el, () => VARS)
    typeInto(el, '@')
    onInput(new Event('input'))
    expect(state.value.open).toBe(true)
  })

  it('邮箱场景（a@b）不触发', () => {
    const el = makeInput('ab')
    const { state, onInput } = useVariablePicker(() => el, () => VARS)
    typeInto(el, '@')
    onInput(new Event('input'))
    expect(state.value.open).toBe(false)
  })

  it('输入元素缺席时 onInput 安全返回', () => {
    const { state, onInput } = useVariablePicker(() => null, () => VARS)
    onInput(new Event('input'))
    expect(state.value.open).toBe(false)
  })

  it('选择器打开后继续输入更新 query；含空白关闭；光标回退到 @ 之前关闭', () => {
    const el = makeInput('')
    const { state, onInput } = useVariablePicker(() => el, () => VARS)
    typeInto(el, '@')
    onInput(new Event('input'))
    expect(state.value.open).toBe(true)

    typeInto(el, 'user')
    onInput(new Event('input'))
    expect(state.value.query).toBe('user')
    expect(state.value.matches.map(m => m.value)).toEqual(['user_input'])

    typeInto(el, ' ')
    onInput(new Event('input'))
    expect(state.value.open).toBe(false)

    // 重新打开后光标回退到 @（startIndex=2）之前 → 关闭
    el.value = 'ab@user'
    el.selectionStart = el.selectionEnd = 0
    state.value.open = true
    state.value.startIndex = 2
    onInput(new Event('input'))
    expect(state.value.open).toBe(false)
  })
})

describe('useVariablePicker 过滤', () => {
  it('query 按 value 与 label 双向匹配，上限 50', () => {
    const el = makeInput('')
    const { state, onInput } = useVariablePicker(() => el, () => VARS)
    typeInto(el, '@')
    onInput(new Event('input'))
    state.value.query = '抓取'
    state.value.matches = (state.value as any)
    // 直接经 onInput 走一遍以触达 computeMatches 的 label 分支
    el.value = '@抓取'
    el.selectionStart = el.selectionEnd = el.value.length
    onInput(new Event('input'))
    expect(state.value.matches.map(m => m.value)).toEqual(['fetch.output.text'])
  })

  it('空 query 返回全部候选（上限 50）', () => {
    const many: VariableOption[] = Array.from({ length: 60 }, (_, i) => ({ value: `v${i}`, label: `V${i}` }))
    const el = makeInput('')
    const { state, onInput } = useVariablePicker(() => el, () => many)
    typeInto(el, '@')
    onInput(new Event('input'))
    expect(state.value.matches.length).toBe(50)
  })
})

describe('useVariablePicker 键盘与插入', () => {
  function openPicker(el: HTMLTextAreaElement) {
    const picker = useVariablePicker(() => el, () => VARS)
    typeInto(el, '@')
    picker.onInput(new Event('input'))
    return picker
  }

  it('ArrowDown/Up 在边界内移动 selectedIndex', () => {
    const el = makeInput('')
    const { state, onKeyDown } = openPicker(el)
    const down = new KeyboardEvent('keydown', { key: 'ArrowDown' })
    onKeyDown(down)
    expect(state.value.selectedIndex).toBe(1)
    onKeyDown(new KeyboardEvent('keydown', { key: 'ArrowDown' }))
    onKeyDown(new KeyboardEvent('keydown', { key: 'ArrowDown' }))
    expect(state.value.selectedIndex).toBe(2)
    onKeyDown(new KeyboardEvent('keydown', { key: 'ArrowUp' }))
    expect(state.value.selectedIndex).toBe(1)
    // 上边界不再减
    for (let i = 0; i < 5; i++) onKeyDown(new KeyboardEvent('keydown', { key: 'ArrowUp' }))
    expect(state.value.selectedIndex).toBe(0)
    void down
  })

  it('Enter 插入 {{var}} 并替换 @、光标落位、v-model 感知、选择器关闭', () => {
    const el = makeInput('hello ')
    const { state, onKeyDown } = openPicker(el)
    typeInto(el, 'user')
    // 手动同步 query（typeInto 已推进光标）
    state.value.query = 'user'
    el.value = 'hello @user'
    el.selectionStart = el.selectionEnd = el.value.length

    let inputFired = 0
    el.addEventListener('input', () => inputFired++)

    const ev = new KeyboardEvent('keydown', { key: 'Enter', cancelable: true })
    onKeyDown(ev)
    expect(ev.defaultPrevented).toBe(true)
    expect(el.value).toBe('hello {{user_input}}')
    expect(el.selectionStart).toBe('hello {{user_input}}'.length)
    expect(inputFired).toBe(1)
    expect(state.value.open).toBe(false)
  })

  it('Enter 无匹配时关闭选择器；Escape 关闭；未打开时按键直通', () => {
    const el = makeInput('')
    const { state, onKeyDown } = openPicker(el)
    state.value.matches = []
    onKeyDown(new KeyboardEvent('keydown', { key: 'Enter', cancelable: true }))
    expect(state.value.open).toBe(false)

    const el2 = makeInput('')
    const p2 = openPicker(el2)
    const esc = new KeyboardEvent('keydown', { key: 'Escape', cancelable: true })
    p2.onKeyDown(esc)
    expect(esc.defaultPrevented).toBe(true)
    expect(p2.state.value.open).toBe(false)

    const el3 = makeInput('')
    const p3 = useVariablePicker(() => el3, () => VARS)
    const ev = new KeyboardEvent('keydown', { key: 'Enter', cancelable: true })
    p3.onKeyDown(ev)
    expect(ev.defaultPrevented).toBe(false)
  })

  it('insert 在元素缺席时仅关闭', () => {
    const { state, insert } = useVariablePicker(() => null, () => VARS)
    state.value.open = true
    insert('x')
    expect(state.value.open).toBe(false)
  })
})
