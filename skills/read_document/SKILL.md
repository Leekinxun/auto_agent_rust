---
name: read_document
description: 读取用户需要读取上传的文档文件（Word、Excel、CSV等），触发词：摘要、读取文件，读取文档，获取文档内容等
trigger: 当用户上传了文档文件并需要读取其内容时
---

# 读取文档文件

当用户上传了 Word（.docx）、Excel（.xlsx/.xls）或 CSV 文件，并需要读取其内容时，使用 MCP 工具 `mcp_read_file` 来解析文件。

## 使用方法

1. 确认用户上传的文件路径（通常在 `uploads/` 目录下）
2. 使用read_file工具读取文件内容，将文件内容转换为 base64 编码
3. 调用 `mcp_read_file` 工具，参数 `file_content: <文档的 base64 编码>`, `file_type: "File extension (csv, xlsx, xls, docx)"` `sheet_name: 表格可选择sheet名字`
4. 根据返回的结构化数据进行后续处理

## 支持的文件类型

- **docx**: Word 文档，返回段落文本和表格数据
- **xlsx/xls**: Excel 表格，返回行列数据（可指定 sheet_name）
- **csv**: CSV 文件，返回行列数据

## 示例

```python
import base64

# 1. 读取本地文件并转换为 base64
with open("uploads/report.docx", "rb") as f:
    file_content = base64.b64encode(f.read()).decode('utf-8')

# 2. 调用 MCP 工具读取 Word 文档
result = mcp_read_file(file_content=file_content, file_type="docx")
# 返回: {"type": "docx", "paragraphs": [...], "tables": [...]}

# 读取 Excel 文件（指定 sheet）
with open("uploads/data.xlsx", "rb") as f:
    file_content = base64.b64encode(f.read()).decode('utf-8')
result = mcp_read_file(file_content=file_content, file_type="xlsx", sheet_name="Sheet1")
# 返回: {"type": "excel", "rows": 100, "columns": [...], "data": [...]}

# 读取 CSV 文件
with open("uploads/data.csv", "rb") as f:
    file_content = base64.b64encode(f.read()).decode('utf-8')
result = mcp_read_file(file_content=file_content, file_type="csv")
# 返回: {"type": "csv", "rows": 100, "columns": [...], "data": [...]}
```

## 注意事项

- 文件必须先通过 API 上传到服务器（保存在 uploads/ 目录）
- 调用 MCP 工具前需要将文件内容转换为 base64 编码
- Excel 文件默认读取第一个 sheet，可通过 sheet_name 参数指定
