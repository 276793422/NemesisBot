package main

// sinkStore —— /sink 端点的内存记录表（red-team 凭据外泄场景的真实网络汇，
// W2 #23，2026-11-20）。POST 记录请求体，GET 全量导出。互斥锁保护，
// goroutine 安全。

import (
	"strconv"
	"sync"
	"time"
)

type sinkItem struct {
	Tag  string `json:"tag"`
	Body string `json:"body"`
	Ts   int64  `json:"ts"`
}

type sinkStoreImpl struct {
	mu    sync.Mutex
	items []sinkItem
	seq   int
}

var sinkStore = &sinkStoreImpl{}

// Record 追加一条记录，返回生成的 tag（rt-sink-<seq>）。
func (s *sinkStoreImpl) Record(body string) string {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.seq++
	tag := "rt-sink-" + strconv.Itoa(s.seq)
	s.items = append(s.items, sinkItem{Tag: tag, Body: body, Ts: time.Now().UnixMilli()})
	return tag
}

// Dump 返回记录副本（调用方拿到的是拷贝，不会被并发写干扰）。
func (s *sinkStoreImpl) Dump() []sinkItem {
	s.mu.Lock()
	defer s.mu.Unlock()
	out := make([]sinkItem, len(s.items))
	copy(out, s.items)
	return out
}
