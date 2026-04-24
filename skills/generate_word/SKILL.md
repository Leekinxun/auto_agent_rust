---
name: generate_word
description: 当且仅当用户要求生成格式规范的中文 Word 文档，其余时候不要调用
trigger: 当且仅当用户要求生成 Word 文档或正式报告时才进行调用
---

# 生成 Word 文档

当用户需要生成 Word 文档时，使用 `python-docx` 库创建格式规范的中文文档。

## 标准格式要求

1. **标题层级**：使用 Heading 1/2/3 样式
2. **正文**：使用 Normal 样式，首行缩进 2 字符
3. **字体**：中文使用宋体或微软雅黑，英文使用 Times New Roman
4. **字号**：标题 16-22 磅，正文 12 磅
5. **行距**：1.5 倍行距
6. **页边距**：上下 2.54cm，左右 3.17cm

## 代码模板

```python
from docx import Document
from docx.shared import Pt, Cm, RGBColor
from docx.enum.text import WD_PARAGRAPH_ALIGNMENT
from docx.oxml.ns import qn

def create_word_document(filename, title, content_sections):
    """
    创建格式规范的中文 Word 文档
    
    Args:
        filename: 输出文件名（如 "报告.docx"）
        title: 文档标题
        content_sections: 内容章节列表，格式：
            [{"heading": "章节标题", "paragraphs": ["段落1", "段落2"]}]
    """
    doc = Document()
    
    # 设置中文字体
    doc.styles['Normal'].font.name = '宋体'
    doc.styles['Normal']._element.rPr.rFonts.set(qn('w:eastAsia'), '宋体')
    doc.styles['Normal'].font.size = Pt(12)
    
    # 添加标题
    title_para = doc.add_heading(title, level=0)
    title_para.alignment = WD_PARAGRAPH_ALIGNMENT.CENTER
    
    # 添加内容章节
    for section in content_sections:
        # 章节标题
        doc.add_heading(section['heading'], level=1)
        
        # 段落内容
        for para_text in section['paragraphs']:
            para = doc.add_paragraph(para_text)
            para.paragraph_format.first_line_indent = Cm(0.74)  # 首行缩进2字符
            para.paragraph_format.line_spacing = 1.5  # 1.5倍行距
    
    # 保存文档
    doc.save(filename)
    return filename

# 使用示例
sections = [
    {
        "heading": "一、项目概述",
        "paragraphs": [
            "本项目旨在...",
            "项目的主要目标包括..."
        ]
    },
    {
        "heading": "二、技术方案",
        "paragraphs": [
            "技术架构采用...",
            "核心模块包括..."
        ]
    }
]

filename = create_word_document("项目报告.docx", "项目技术报告", sections)
print(f"文档已生成：{filename}")
```

## 注意事项

- 文件保存在当前工作目录，用户可通过下载接口获取
- 标题使用中文序号（一、二、三）
- 段落之间自动添加适当间距
- 支持添加表格、图片等元素（根据需要扩展）
- 输出的结果中不要带有本地的文件路径
