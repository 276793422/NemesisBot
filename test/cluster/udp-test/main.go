// Simple UDP broadcast test tool
package main

import (
	"encoding/json"
	"fmt"
	"net"
	"os"
	"time"
)

type TestMessage struct {
	Message string `json:"message"`
	Time    int64  `json:"time"`
}

func main() {
	if len(os.Args) < 2 {
		fmt.Println("Usage: go run cmd/udp-test/main.go <send|receive> [port]")
		os.Exit(1)
	}

	mode := os.Args[1]
	port := "49100"
	if len(os.Args) > 2 {
		port = os.Args[2]
	}

	if mode == "send" {
		testSend(port)
	} else if mode == "receive" {
		testReceive(port)
	} else {
		fmt.Println("Invalid mode. Use 'send' or 'receive'")
		os.Exit(1)
	}
}

func testSend(port string) {
	fmt.Printf("Starting UDP sender on port %s...\n", port)

	// Create UDP connection
	conn, err := net.Dial("udp", fmt.Sprintf("255.255.255.255:%s", port))
	if err != nil {
		fmt.Printf("Error dialing: %v\n", err)
		os.Exit(1)
	}
	defer conn.Close()

	// Also try localhost
	localConn, err := net.Dial("udp", fmt.Sprintf("127.0.0.1:%s", port))
	if err != nil {
		fmt.Printf("Error dialing localhost: %v\n", err)
	} else {
		defer localConn.Close()
	}

	msg := &TestMessage{
		Message: "Hello from sender!",
		Time:    time.Now().Unix(),
	}

	data, _ := json.Marshal(msg)

	fmt.Printf("Sending to broadcast 255.255.255.255:%s\n", port)
	for i := 0; i < 5; i++ {
		_, err := conn.Write(data)
		if err != nil {
			fmt.Printf("Error sending (broadcast): %v\n", err)
		} else {
			fmt.Printf("Sent message %d via broadcast\n", i+1)
		}

		if localConn != nil {
			_, err := localConn.Write(data)
			if err != nil {
				fmt.Printf("Error sending (localhost): %v\n", err)
			} else {
				fmt.Printf("Sent message %d via localhost\n", i+1)
			}
		}

		time.Sleep(1 * time.Second)
	}

	fmt.Println("Done sending")
}

func testReceive(port string) {
	fmt.Printf("Starting UDP receiver on port %s...\n", port)

	// Listen on all interfaces
	addr, err := net.ResolveUDPAddr("udp", fmt.Sprintf(":%s", port))
	if err != nil {
		fmt.Printf("Error resolving: %v\n", err)
		os.Exit(1)
	}

	conn, err := net.ListenUDP("udp", addr)
	if err != nil {
		fmt.Printf("Error listening: %v\n", err)
		os.Exit(1)
	}
	defer conn.Close()

	fmt.Printf("Listening on %s\n", conn.LocalAddr())

	buf := make([]byte, 1024)
	receivedCount := 0

	// Set deadline to avoid blocking forever
	conn.SetReadDeadline(time.Now().Add(30 * time.Second))

	for receivedCount < 10 {
		n, src, err := conn.ReadFromUDP(buf)
		if err != nil {
			if netErr, ok := err.(net.Error); ok && netErr.Timeout() {
				fmt.Println("Timeout waiting for messages")
				break
			}
			fmt.Printf("Error receiving: %v\n", err)
			continue
		}

		receivedCount++

		var msg TestMessage
		if err := json.Unmarshal(buf[:n], &msg); err != nil {
			fmt.Printf("[%d] Received non-JSON data from %s: %s\n", receivedCount, src, string(buf[:n]))
		} else {
			fmt.Printf("[%d] Received from %s: %s\n", receivedCount, src, msg.Message)
		}
	}

	fmt.Printf("Received %d messages total\n", receivedCount)
}
