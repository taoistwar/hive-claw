-- V026__seed_sensitive_words
-- 预置基础敏感词库（精确匹配 + 正则匹配），INSERT IGNORE 防重复
INSERT IGNORE INTO sensitive_words (word, match_mode) VALUES
('赌博', 'exact'), ('赌场', 'exact'), ('赌钱', 'exact'), ('博彩', 'exact'),
('彩票', 'exact'), ('赌球', 'exact'), ('六合彩', 'exact'), ('老虎机', 'exact'),
('百家乐', 'exact'), ('轮盘赌', 'exact'), ('时时彩', 'exact'),
('地下赌场', 'exact'), ('网赌', 'exact'), ('线上赌场', 'exact'),
('色情', 'exact'), ('黄色', 'exact'), ('成人影片', 'exact'), ('成人网站', 'exact'),
('色情网站', 'exact'), ('裸聊', 'exact'), ('约炮', 'exact'), ('一夜情', 'exact'),
('卖淫', 'exact'), ('嫖娼', 'exact'),
('吸毒', 'exact'), ('毒品', 'exact'), ('白粉', 'exact'), ('冰毒', 'exact'),
('摇头丸', 'exact'), ('大麻', 'exact'), ('海洛因', 'exact'), ('贩卖毒品', 'exact'),
('枪支', 'exact'), ('军火', 'exact'), ('炸药', 'exact'), ('法轮功', 'exact'),
('高利贷', 'exact'), ('洗钱', 'exact'),
('偷渡', 'exact'), ('走私', 'exact'), ('假币', 'exact'), ('假钞', 'exact'),
('迷药', 'exact'), ('迷魂药', 'exact'), ('春药', 'exact'),
('porn', 'exact'), ('bomb', 'exact'), ('gun', 'exact'),
('heroin', 'exact'), ('cocaine', 'exact'), ('cannabis', 'exact'),
('gambling', 'exact'), ('casino', 'exact'), ('prostitute', 'exact'),
;

-- 正则匹配：手机号、身份证号、邮箱、URL、微信号、脏话变体
INSERT IGNORE INTO sensitive_words (word, match_mode) VALUES
('草.*泥.*马', 'regex'),