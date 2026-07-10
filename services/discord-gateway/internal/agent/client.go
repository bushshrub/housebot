package agent

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"net/http"
	"time"
)

type Client struct {
	baseURL    string
	httpClient *http.Client
}

func NewClient(baseURL string) *Client {
	return &Client{
		baseURL: baseURL,
		httpClient: &http.Client{Timeout: 5 * time.Minute},
	}
}

type RunRequest struct {
	UserID      string      `json:"user_id"`
	Username    string      `json:"username"`
	Text        string      `json:"text"`
	Images      []ImageData `json:"images,omitempty"`
	Personality *string     `json:"personality,omitempty"`
}

type ImageData struct {
	MediaType string `json:"media_type"`
	Data      string `json:"data"`
}

type RunResponse struct {
	Text          string  `json:"text"`
	SessionNotice *string `json:"session_notice,omitempty"`
}

func (c *Client) Run(req RunRequest) (*RunResponse, error) {
	body, err := json.Marshal(req)
	if err != nil {
		return nil, err
	}
	resp, err := c.httpClient.Post(c.baseURL+"/run", "application/json", bytes.NewReader(body))
	if err != nil {
		return nil, fmt.Errorf("agent request failed: %w", err)
	}
	defer resp.Body.Close()
	var result RunResponse
	if err := json.NewDecoder(resp.Body).Decode(&result); err != nil {
		return nil, err
	}
	return &result, nil
}

func (c *Client) Reset(userID string) error {
	resp, err := c.httpClient.Post(
		c.baseURL+"/session/"+userID+"/reset",
		"application/json",
		bytes.NewReader([]byte("{}")),
	)
	if err != nil {
		return err
	}
	resp.Body.Close()
	return nil
}

func (c *Client) Compact(userID string) error {
	resp, err := c.httpClient.Post(
		c.baseURL+"/session/"+userID+"/compact",
		"application/json",
		bytes.NewReader([]byte("{}")),
	)
	if err != nil {
		return err
	}
	resp.Body.Close()
	return nil
}

func (c *Client) ModelInfo() (string, error) {
	resp, err := c.httpClient.Get(c.baseURL + "/model")
	if err != nil {
		return "", err
	}
	defer resp.Body.Close()
	var v struct {
		Info string `json:"info"`
	}
	if err := json.NewDecoder(resp.Body).Decode(&v); err != nil {
		return "", err
	}
	return v.Info, nil
}
