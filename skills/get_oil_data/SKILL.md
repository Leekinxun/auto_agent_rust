---
name: get_oil_data
description: 当用户查询油料相关信息时触发，例如查询3号航煤存储量、发送量等
tags: oil
---

# 油料数据查询 Skill

## 触发条件

用户查询油料相关信息，例如：
- 3号航煤存储量
- 油料发送量
- 油料库存统计

## 执行步骤

1. **获取数据库列表**
   - 调用 `mcp_list_databases`
   - 选择 `oil` 数据库

2. **获取表结构**
   - 调用 `mcp_list_tables`，参数 `dbname: "oil"`
   - 根据用户问题选择相关表

3. **查看表详情**
   - 调用 `mcp_describe_table`，参数 `table_name: <表名>`, `dbname: "oil"`
   - 确认相关列和数据类型

4. **执行查询**
   - 调用 `mcp_query_database`，参数 `sql: <SQL语句>`, `dbname: "oil"`
   - 构建 SQL 时注意：
     - 使用正确的表名和列名
     - 添加必要的 WHERE 条件
     - 使用聚合函数（SUM/COUNT/AVG）处理统计需求

5. **返回结果**
   - 将查询结果以清晰格式返回给用户
   - 必要时添加单位说明（吨、升等）
